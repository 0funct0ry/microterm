//! Placing the skill: a directory we own (Claude Code) or a managed block in a shared file.
//! Installs are idempotent, never touch content we did not write, and report every path.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{render, Agent, Scope, BEGIN, END, SKILL_NAME};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// `<dir>/microterm-ctl/SKILL.md`, owned by us.
    SkillFile,
    /// A managed block inside a shared markdown file.
    Block,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub agent: Agent,
    pub path: PathBuf,
    pub form: Form,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Wrote,
    Unchanged,
    Removed,
    Absent,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Wrote => "wrote",
            Action::Unchanged => "unchanged",
            Action::Removed => "removed",
            Action::Absent => "absent",
        }
    }
}

/// Where the paths are rooted (injected so tests need no environment).
#[derive(Debug, Clone)]
pub struct Roots {
    pub home: PathBuf,
    pub cwd: PathBuf,
    /// `$CODEX_HOME`, default `~/.codex`.
    pub codex_home: Option<PathBuf>,
}

impl Roots {
    pub fn from_env() -> Result<Roots, String> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("$HOME is not set")?;
        let cwd = std::env::current_dir().map_err(|e| format!("no current directory: {e}"))?;
        Ok(Roots {
            home,
            cwd,
            codex_home: std::env::var_os("CODEX_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
        })
    }
}

/// The files for `agent`. Returns the targets and notes to show the user (such as the
/// antigravity fallback). A scope the agent has no location for is an error, except under `all`
/// where it is skipped with a note.
pub fn targets(
    agent: Agent,
    scope: Option<Scope>,
    dir: Option<&Path>,
    roots: &Roots,
) -> Result<(Vec<Target>, Vec<String>), String> {
    let mut notes = Vec::new();
    let mut out: Vec<Target> = Vec::new();
    let explicit = scope.is_some();
    let list: &[Agent] = match agent {
        Agent::All => &[Agent::Claude, Agent::Codex, Agent::Generic],
        Agent::Claude => &[Agent::Claude],
        Agent::Codex => &[Agent::Codex],
        Agent::Antigravity => &[Agent::Antigravity],
        Agent::Generic => &[Agent::Generic],
    };
    for &a in list {
        let sc = scope.unwrap_or(match a {
            Agent::Claude | Agent::Codex => Scope::User,
            _ => Scope::Project,
        });
        let t = match (a, sc) {
            (Agent::Claude, Scope::User) => Target {
                agent: a,
                path: dir
                    .map_or_else(|| roots.home.join(".claude/skills"), Path::to_path_buf)
                    .join(SKILL_NAME)
                    .join("SKILL.md"),
                form: Form::SkillFile,
            },
            (Agent::Claude, Scope::Project) => Target {
                agent: a,
                path: dir
                    .map_or_else(|| roots.cwd.join(".claude/skills"), Path::to_path_buf)
                    .join(SKILL_NAME)
                    .join("SKILL.md"),
                form: Form::SkillFile,
            },
            (Agent::Codex, Scope::User) => Target {
                agent: a,
                path: dir
                    .map(Path::to_path_buf)
                    .or_else(|| roots.codex_home.clone())
                    .unwrap_or_else(|| roots.home.join(".codex"))
                    .join("AGENTS.md"),
                form: Form::Block,
            },
            (Agent::Codex | Agent::Generic | Agent::Antigravity, Scope::Project) => Target {
                agent: a,
                path: dir.unwrap_or(&roots.cwd).join("AGENTS.md"),
                form: Form::Block,
            },
            (Agent::Generic | Agent::Antigravity, Scope::User) => {
                if agent == Agent::All && !explicit {
                    continue;
                }
                return Err(format!(
                    "{} has no user-scope location; use --scope project (or --dir)",
                    if a == Agent::Generic {
                        "generic"
                    } else {
                        "antigravity"
                    }
                ));
            }
            (Agent::All, _) => unreachable!(),
        };
        if a == Agent::Antigravity {
            notes.push("antigravity: its instruction location could not be verified against Google's documentation, so this writes the generic AGENTS.md block (see docs/adr/0001-agent-integration.md)".into());
        }
        if !out.iter().any(|o| o.path == t.path) {
            out.push(t);
        }
    }
    Ok((out, notes))
}

fn content(t: &Target) -> String {
    match t.form {
        Form::SkillFile => render::claude_skill(),
        Form::Block => render::managed_block(),
    }
}

fn read(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Temp file + rename, so a reader never sees a half-written instruction file.
fn write_atomic(path: &Path, data: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Byte range of our block (markers included, plus the newline after the end marker).
fn block_range(s: &str) -> Option<(usize, usize)> {
    let b = s.find(BEGIN)?;
    let e = s[b..].find(END)? + b + END.len();
    let e = if s[e..].starts_with('\n') { e + 1 } else { e };
    Some((b, e))
}

pub fn install(t: &Target) -> Result<Action, String> {
    let new = content(t);
    let existing = read(&t.path).map_err(|e| format!("{}: {e}", t.path.display()))?;
    let result = match (t.form, existing.as_deref()) {
        (Form::SkillFile, Some(cur)) => {
            if cur != new && !cur.contains(BEGIN) {
                return Err(format!(
                    "{} exists and was not written by microterm; refusing to overwrite it",
                    t.path.display()
                ));
            }
            new
        }
        (Form::SkillFile, None) => new,
        (Form::Block, None) => new,
        (Form::Block, Some(cur)) => match block_range(cur) {
            Some((b, e)) => format!("{}{}{}", &cur[..b], new, &cur[e..]),
            None if cur.is_empty() => new,
            None => {
                let mut s = cur.to_string();
                if !s.ends_with('\n') {
                    s.push('\n');
                }
                s.push('\n');
                s.push_str(&new);
                s
            }
        },
    };
    if existing.as_deref() == Some(result.as_str()) {
        return Ok(Action::Unchanged);
    }
    write_atomic(&t.path, &result).map_err(|e| format!("{}: {e}", t.path.display()))?;
    Ok(Action::Wrote)
}

pub fn uninstall(t: &Target) -> Result<Action, String> {
    let err = |e: io::Error| format!("{}: {e}", t.path.display());
    let Some(cur) = read(&t.path).map_err(err)? else {
        return Ok(Action::Absent);
    };
    match t.form {
        Form::SkillFile => {
            if !cur.contains(BEGIN) {
                return Err(format!(
                    "{} was not written by microterm; leaving it alone",
                    t.path.display()
                ));
            }
            fs::remove_file(&t.path).map_err(err)?;
            if let Some(dir) = t.path.parent() {
                let _ = fs::remove_dir(dir); // only if now empty
            }
            Ok(Action::Removed)
        }
        Form::Block => {
            let Some((b, e)) = block_range(&cur) else {
                return Ok(Action::Absent);
            };
            let mut head = cur[..b].to_string();
            if head.ends_with("\n\n") {
                head.pop(); // the separator install added
            }
            let rest = format!("{head}{}", &cur[e..]);
            if rest.trim().is_empty() {
                fs::remove_file(&t.path).map_err(err)?;
            } else {
                write_atomic(&t.path, &rest).map_err(err)?;
            }
            Ok(Action::Removed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(tag: &str) -> Roots {
        let base = std::env::temp_dir().join(format!("mt-skill-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("home")).unwrap();
        fs::create_dir_all(base.join("proj")).unwrap();
        Roots {
            home: base.join("home"),
            cwd: base.join("proj"),
            codex_home: None,
        }
    }

    fn one(agent: Agent, scope: Option<Scope>, r: &Roots) -> Target {
        targets(agent, scope, None, r).unwrap().0.remove(0)
    }

    #[test]
    fn claude_paths_and_form() {
        let r = roots("paths");
        let u = one(Agent::Claude, None, &r);
        assert_eq!(u.path, r.home.join(".claude/skills/microterm-ctl/SKILL.md"));
        let p = one(Agent::Claude, Some(Scope::Project), &r);
        assert_eq!(p.path, r.cwd.join(".claude/skills/microterm-ctl/SKILL.md"));
        let c = one(Agent::Codex, None, &r);
        assert_eq!(c.path, r.home.join(".codex/AGENTS.md"));
        let mut r2 = r.clone();
        r2.codex_home = Some(r.home.join("ch"));
        assert_eq!(
            one(Agent::Codex, None, &r2).path,
            r.home.join("ch/AGENTS.md")
        );
        assert_eq!(one(Agent::Generic, None, &r).path, r.cwd.join("AGENTS.md"));
        assert!(targets(Agent::Generic, Some(Scope::User), None, &r).is_err());
    }

    #[test]
    fn antigravity_falls_back_to_generic_with_a_note() {
        let r = roots("ag");
        let (t, notes) = targets(Agent::Antigravity, None, None, &r).unwrap();
        assert_eq!(t[0].path, r.cwd.join("AGENTS.md"));
        assert!(notes[0].contains("antigravity"));
    }

    #[test]
    fn all_dedups_and_skips_generic_in_user_scope() {
        let r = roots("all");
        let (t, _) = targets(Agent::All, Some(Scope::Project), None, &r).unwrap();
        assert_eq!(t.len(), 2, "codex and generic share ./AGENTS.md: {t:?}");
        let (t, _) = targets(Agent::All, None, None, &r).unwrap();
        assert_eq!(t.len(), 3); // claude user, codex user, generic project
        assert!(targets(Agent::All, Some(Scope::User), None, &r).is_err());
    }

    #[test]
    fn dir_override() {
        let r = roots("dir");
        let d = r.cwd.join("x");
        let (t, _) = targets(Agent::Claude, None, Some(&d), &r).unwrap();
        assert_eq!(t[0].path, d.join("microterm-ctl/SKILL.md"));
        let (t, _) = targets(Agent::Generic, None, Some(&d), &r).unwrap();
        assert_eq!(t[0].path, d.join("AGENTS.md"));
    }

    #[test]
    fn skill_file_install_is_idempotent_and_uninstalls_its_directory() {
        let r = roots("skill");
        let t = one(Agent::Claude, None, &r);
        assert_eq!(install(&t).unwrap(), Action::Wrote);
        let text = fs::read_to_string(&t.path).unwrap();
        assert!(text.starts_with("---\nname: microterm-ctl\ndescription: "));
        assert_eq!(install(&t).unwrap(), Action::Unchanged);
        assert_eq!(uninstall(&t).unwrap(), Action::Removed);
        assert!(!t.path.parent().unwrap().exists());
        assert!(r.home.join(".claude/skills").exists(), "parent kept");
        assert_eq!(uninstall(&t).unwrap(), Action::Absent);
    }

    #[test]
    fn foreign_skill_file_is_never_overwritten_or_removed() {
        let r = roots("foreign-skill");
        let t = one(Agent::Claude, None, &r);
        fs::create_dir_all(t.path.parent().unwrap()).unwrap();
        fs::write(&t.path, "mine\n").unwrap();
        assert!(install(&t).is_err());
        assert!(uninstall(&t).is_err());
        assert_eq!(fs::read_to_string(&t.path).unwrap(), "mine\n");
    }

    #[test]
    fn block_preserves_foreign_content_and_round_trips() {
        let r = roots("block");
        let t = one(Agent::Generic, None, &r);
        let original = "# Project rules\n\nUse tabs.\n";
        fs::write(&t.path, original).unwrap();
        assert_eq!(install(&t).unwrap(), Action::Wrote);
        let after = fs::read_to_string(&t.path).unwrap();
        assert!(after.starts_with(original));
        assert!(after.contains(BEGIN) && after.contains(END));
        assert_eq!(install(&t).unwrap(), Action::Unchanged);
        assert_eq!(uninstall(&t).unwrap(), Action::Removed);
        assert_eq!(fs::read_to_string(&t.path).unwrap(), original);
    }

    #[test]
    fn block_in_the_middle_is_replaced_in_place_and_new_file_is_deleted_on_uninstall() {
        let r = roots("mid");
        let t = one(Agent::Generic, None, &r);
        fs::write(&t.path, format!("top\n{BEGIN}\nstale\n{END}\nbottom\n")).unwrap();
        assert_eq!(install(&t).unwrap(), Action::Wrote);
        let s = fs::read_to_string(&t.path).unwrap();
        assert!(s.starts_with("top\n") && s.ends_with("bottom\n") && !s.contains("stale"));
        fs::remove_file(&t.path).unwrap();
        install(&t).unwrap();
        uninstall(&t).unwrap();
        assert!(
            !t.path.exists(),
            "a file we created is removed with our block"
        );
    }
}
