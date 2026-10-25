//! Shell-registered and project menu items (SPEC §10.9.7). GPUI-free.
//!
//! Items arrive over `OSC 7717` (see `mt_term::menu_reg`) or from a project menu file (`.microterm-menu.toml` or an alias) in a
//! trusted working directory. Both are untrusted-origin data, so they run under Tier 2 rules:
//! `send` into their own pane, `ctl` restricted to their own pane, `spawn` only from a trusted
//! local directory.

use std::path::Path;

use mt_config::menu::Confirm;
use mt_config::trust;
use mt_config::Config;
use mt_term::menu_reg::RegKind;
use mt_term::Terminal;

use super::{confirm_reason, expand, Facts, Invocation, MenuRow, Plan, PlanError, RowKind};
use mt_config::menu::{Kind, SpawnWhere};

/// Row-id prefixes: `viewport/shell:<id>` and `viewport/project:<id>`.
pub const SHELL_PREFIX: &str = "shell:";
pub const PROJECT_PREFIX: &str = "project:";
/// Row ids of `OSC 7723` bindings and `OSC 7724` palette entries. Never rendered as menu rows.
pub const PANE_KEY_PREFIX: &str = "pane-key:";
pub const PANE_ENTRY_PREFIX: &str = "pane-entry:";
/// Row id of the diagnostic shown when a project file is ignored.
pub const IGNORED_ID: &str = "project:ignored";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Registered over `OSC 7717` by a program in the pane.
    Shell,
    /// Read from a project menu file (`.microterm-menu.toml` or an alias) in a trusted directory.
    Project,
    /// Registered over `OSC 7723`: a chord bound to its own pane.
    PaneKey,
    /// Registered over `OSC 7724`: a "From pane" palette entry.
    PaneEntry,
}

impl Origin {
    pub fn prefix(self) -> &'static str {
        match self {
            Origin::Shell => SHELL_PREFIX,
            Origin::Project => PROJECT_PREFIX,
            Origin::PaneKey => PANE_KEY_PREFIX,
            Origin::PaneEntry => PANE_ENTRY_PREFIX,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Origin::Shell => "shell",
            Origin::Project => "project",
            Origin::PaneKey => "pane-key",
            Origin::PaneEntry => "pane-entry",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellItem {
    pub id: String,
    pub label: String,
    pub kind: RegKind,
    pub payload: String,
    pub origin: Origin,
}

impl ShellItem {
    /// The id the menu addresses it by.
    pub fn row_id(&self) -> String {
        format!("{}{}", self.origin.prefix(), self.id)
    }
}

/// What a pane offers beyond the config: registered items and project items.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellState {
    pub items: Vec<ShellItem>,
    /// A project menu file exists here but the directory is not trusted.
    pub project_ignored: bool,
    /// Name of the project file found in the pane's directory, if any.
    pub project_file: Option<&'static str>,
    /// A project file failed to parse (the message is shown as a disabled row).
    pub project_error: Option<String>,
}

/// Collects the items of a pane. Remote panes only contribute registered items, and only when
/// `allow_remote_items` is set; project files are read for local panes in trusted directories.
pub fn collect(
    cfg: &Config,
    remote: bool,
    cwd: Option<&str>,
    term: Option<&Terminal>,
) -> ShellState {
    let mut st = ShellState::default();
    if !remote || cfg.menu_security.allow_remote_items {
        if let Some(t) = term {
            for r in t.menu_items().0 {
                st.items.push(ShellItem {
                    id: r.id,
                    label: r.label,
                    kind: r.kind,
                    payload: r.payload,
                    origin: Origin::Shell,
                });
            }
            for k in t.pane_keys() {
                st.items.push(ShellItem {
                    label: k.chord.clone(),
                    id: k.id,
                    kind: k.kind,
                    payload: k.payload,
                    origin: Origin::PaneKey,
                });
            }
            for e in t.pane_palette() {
                st.items.push(ShellItem {
                    id: e.id,
                    label: e.label,
                    kind: e.kind,
                    payload: e.payload,
                    origin: Origin::PaneEntry,
                });
            }
        }
    }
    if let (false, Some(cwd)) = (remote, cwd) {
        if let Some((name, text)) = trust::read_project_file(Path::new(cwd)) {
            st.project_file = Some(name);
            if trust::is_trusted(&cfg.menu_security, Path::new(cwd)) {
                match trust::parse_project(&text) {
                    Ok(entries) => {
                        for e in entries {
                            let kind = match e.kind {
                                "send" => RegKind::Send,
                                "ctl" => RegKind::Ctl,
                                _ => RegKind::Spawn,
                            };
                            st.items.push(ShellItem {
                                id: e.id,
                                label: e.label,
                                kind,
                                payload: e.payload,
                                origin: Origin::Project,
                            });
                        }
                    }
                    Err(e) => st.project_error = Some(format!("{name}: {e}")),
                }
            } else {
                st.project_ignored = true;
            }
        }
    }
    st
}

/// Whether the pane's cwd (from the facts) is a trusted local directory right now.
pub fn trusted(cfg: &Config, f: &Facts) -> bool {
    !f.remote
        && f.values
            .get("cwd")
            .is_some_and(|c| trust::is_trusted(&cfg.menu_security, Path::new(c)))
}

fn usable(cfg: &Config, f: &Facts, it: &ShellItem) -> Result<(), String> {
    if it.kind == RegKind::Spawn && !trusted(cfg, f) {
        return Err("directory not trusted (see [menu_security] trusted_dirs)".into());
    }
    Ok(())
}

fn row(id: String, label: String, enabled: bool) -> MenuRow {
    MenuRow {
        id,
        label,
        chip_action: None,
        icon: Some("terminal".into()),
        enabled,
        kind: RowKind::Item,
    }
}

fn header(id: &str, label: &str) -> MenuRow {
    MenuRow {
        id: id.into(),
        label: label.into(),
        chip_action: None,
        icon: None,
        enabled: false,
        kind: RowKind::Header,
    }
}

/// The rows appended to `viewport`, after the config rows.
pub fn rows(cfg: &Config, f: &Facts) -> Vec<MenuRow> {
    let mut out = Vec::new();
    for origin in [Origin::Shell, Origin::Project] {
        let items: Vec<&ShellItem> = f
            .shell
            .items
            .iter()
            .filter(|i| i.origin == origin)
            .collect();
        let extra = origin == Origin::Project
            && (f.shell.project_ignored || f.shell.project_error.is_some());
        if items.is_empty() && !extra {
            continue;
        }
        out.push(match (origin, f.remote) {
            (Origin::Shell, true) => header("shell-header", "Shell (remote)"),
            (Origin::Shell, false) => header("shell-header", "Shell"),
            (Origin::Project, _) => header("project-header", "Project"),
            (Origin::PaneKey | Origin::PaneEntry, _) => continue,
        });
        for it in items {
            let label: String = it.label.chars().filter(|c| !c.is_control()).collect();
            out.push(row(it.row_id(), label, usable(cfg, f, it).is_ok()));
        }
        if origin == Origin::Project {
            if f.shell.project_ignored {
                out.push(row(
                    IGNORED_ID.into(),
                    format!(
                        "{} ignored: directory not trusted",
                        f.shell.project_file.unwrap_or(trust::PROJECT_FILES[0])
                    ),
                    false,
                ));
            } else if let Some(e) = &f.shell.project_error {
                out.push(row(IGNORED_ID.into(), e.chars().take(120).collect(), false));
            }
        }
    }
    if let Some(text) = &f.program_badge {
        out.push(header(
            "badge-header",
            &format!("Badge \"{text}\" set by a program"),
        ));
    }
    out
}

/// The origin marker shown as the row's chip, and in `menu show --json`.
pub fn origin_of(row_id: &str) -> Option<&'static str> {
    if row_id.starts_with(SHELL_PREFIX) {
        Some("shell")
    } else if row_id.starts_with(PROJECT_PREFIX) {
        Some("project")
    } else if row_id.starts_with(PANE_KEY_PREFIX) {
        Some("pane-key")
    } else if row_id.starts_with(PANE_ENTRY_PREFIX) {
        Some("pane-entry")
    } else {
        None
    }
}

/// Words that would point a `ctl` verb at something other than the item's own pane.
const TARGET_FLAGS: [&str; 6] = [
    "--pane",
    "--match",
    "--tab",
    "--window",
    "--workspace",
    "--all",
];

fn check_ctl(argv: &[String]) -> Result<(), String> {
    if argv.first().map(String::as_str) != Some("pane") {
        return Err("a shell-registered ctl item may only run `pane` verbs".into());
    }
    for w in argv {
        let flag = w.split('=').next().unwrap_or(w);
        if TARGET_FLAGS.contains(&flag) {
            return Err(format!(
                "{flag} is not allowed in a shell-registered item (own pane only)"
            ));
        }
    }
    Ok(())
}

/// Plans `viewport/shell:<id>` or `viewport/project:<id>` (Tier 2, SPEC §10.9.7).
pub fn plan(cfg: &Config, menu: &str, item: &str, f: &Facts) -> Result<Invocation, PlanError> {
    let addr = format!("{menu}/{item}");
    let it = f
        .shell
        .items
        .iter()
        .find(|i| i.row_id() == item)
        .ok_or_else(|| PlanError::UnknownItem(addr.clone()))?;
    usable(cfg, f, it).map_err(|e| PlanError::Unavailable(format!("{addr}: {e}")))?;
    if matches!(it.origin, Origin::PaneKey | Origin::PaneEntry) && it.kind == RegKind::Spawn {
        return Err(PlanError::Unavailable(format!(
            "{addr}: spawn is not available to pane key bindings or palette entries"
        )));
    }
    let bad = |m: String| PlanError::Expand(format!("{addr}: {m}"));
    let (kind, plan) = match it.kind {
        RegKind::Send => {
            if it.payload.contains(['\n', '\r']) {
                return Err(bad("text contains a newline".into()));
            }
            let mut text = expand::sanitize(&it.payload, false);
            text.push('\n');
            (Kind::Send, Plan::Send { text })
        }
        RegKind::Ctl => {
            let words = mt_config::split_command(&it.payload).map_err(bad)?;
            let argv = expand::expand_argv(
                &words
                    .iter()
                    .map(|w| w.replace('{', "{{").replace('}', "}}"))
                    .collect::<Vec<_>>(),
                &f.values,
            )
            .map_err(|e| bad(e.to_string()))?;
            check_ctl(&argv).map_err(bad)?;
            (Kind::Ctl, Plan::Ctl { argv })
        }
        RegKind::Spawn => {
            let argv: Vec<String> = serde_json::from_str(&it.payload)
                .map_err(|e| bad(format!("spawn payload is not a JSON string array: {e}")))?;
            if argv.is_empty() {
                return Err(bad("spawn payload is empty".into()));
            }
            let argv = argv.iter().map(|a| expand::sanitize(a, false)).collect();
            (
                Kind::Spawn,
                Plan::Spawn {
                    argv,
                    cwd: f.values.get("cwd").map(str::to_string),
                    title: Some(it.label.chars().take(40).collect()),
                    place: SpawnWhere::SplitRight,
                    close_on_success: false,
                    ctl: false,
                    env: expand::env_for(&f.values, menu, item),
                },
            )
        }
    };
    let confirm = confirm_reason(cfg, Confirm::Auto, kind, &plan, f, false);
    Ok(Invocation {
        menu: menu.to_string(),
        item: item.to_string(),
        pane: f.pane,
        kind: Some(kind),
        plan,
        confirm,
    })
}
