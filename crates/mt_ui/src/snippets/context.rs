//! Answers template facts for one pane (SPEC §10.8.4). The same type serves the GUI (with a
//! live terminal, clipboard and window state) and the headless server (mux snapshot only,
//! where GUI-only facts fail and `default=` takes over).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mt_config::Config;
use mt_mux::ctl::Rows;
use mt_snippets::chrono::{DateTime, FixedOffset};
use mt_snippets::{system, Context, Fact};
use mt_term::Terminal;

/// Per-window counters (SPEC §10.8.4 `counter`), in memory only.
static COUNTERS: Mutex<Option<HashMap<(u64, String), i64>>> = Mutex::new(None);

pub struct PaneContext {
    pub cfg: Arc<Config>,
    pub rows: Rows,
    pub pane: Option<u64>,
    pub term: Option<Arc<Terminal>>,
    /// Clipboard text captured when the context was built (`None` = no clipboard, e.g. headless).
    pub clipboard: Option<String>,
    /// Effective font size including the window's own zoom (`None` when headless).
    pub font_size: Option<f32>,
    /// Theme name (`None` when headless).
    pub theme: Option<String>,
}

impl PaneContext {
    fn row(&self) -> Result<&mt_mux::ctl::PaneRow, String> {
        let id = self.pane.ok_or("no target pane")?;
        self.rows
            .panes
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("pane {id} no longer exists"))
    }

    fn window_key(&self) -> u64 {
        self.row().map_or(0, |r| r.window)
    }

    fn gui_only(&self, what: &str) -> Result<(), String> {
        Err(format!("{what} is not available in a headless instance"))
    }
}

impl Context for PaneContext {
    fn now(&self) -> DateTime<FixedOffset> {
        system::now()
    }

    fn fill_random(&mut self, buf: &mut [u8]) {
        system::os_random(buf);
    }

    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn fact(&mut self, fact: &Fact) -> Result<String, String> {
        if let Some(r) = system::system_fact(fact) {
            return r;
        }
        match fact {
            Fact::Shell => Ok(mt_term::resolve_shell(&self.cfg.shell)),
            Fact::Version => Ok(env!("CARGO_PKG_VERSION").to_string()),
            Fact::Cwd => {
                let row = self.row()?;
                row.cwd
                    .clone()
                    .or_else(|| {
                        self.term.as_ref().and_then(|t| {
                            t.reported_cwd()
                                .or_else(|| t.cwd().map(|p| p.to_string_lossy().into_owned()))
                        })
                    })
                    .ok_or_else(|| "the pane's working directory is unknown".to_string())
            }
            Fact::IsRemote => Ok((self.row()?.domain != "local").to_string()),
            Fact::PaneId => Ok(self.row()?.id.to_string()),
            Fact::TabId => Ok(self.row()?.tab.to_string()),
            Fact::WindowId => Ok(self.row()?.window.to_string()),
            Fact::TabIndex => {
                let row = self.row()?;
                self.rows
                    .windows
                    .iter()
                    .find(|w| w.id == row.window)
                    .and_then(|w| w.tabs.iter().position(|t| *t == row.tab))
                    .map(|i| (i + 1).to_string())
                    .ok_or_else(|| "the pane's tab is unknown".to_string())
            }
            Fact::TabTitle => {
                let tab = self.row()?.tab;
                self.rows
                    .tabs
                    .iter()
                    .find(|t| t.id == tab)
                    .map(|t| t.title.clone())
                    .ok_or_else(|| "the pane's tab is unknown".to_string())
            }
            Fact::PaneTitle => Ok(self.row()?.title.clone()),
            Fact::Workspace => Ok(self.row()?.workspace.clone()),
            Fact::Domain => Ok(self.row()?.domain.clone()),
            Fact::RemoteHost => {
                let domain = self.row()?.domain.clone();
                if domain == "local" {
                    return Err("the pane is local".into());
                }
                self.cfg
                    .ssh_domains
                    .iter()
                    .find(|d| d.name == domain)
                    .map(|d| d.host.clone())
                    .ok_or_else(|| format!("domain \"{domain}\" is not an SSH domain"))
            }
            Fact::Cols => Ok(self.row()?.size.0.to_string()),
            Fact::Rows => Ok(self.row()?.size.1.to_string()),
            Fact::FontFamily => match self.font_size {
                Some(_) => Ok(self.cfg.font.family.clone()),
                None => self.gui_only("font_family").map(|()| String::new()),
            },
            Fact::FontSize => match self.font_size {
                Some(s) => Ok(format!("{s}")),
                None => self.gui_only("font_size").map(|()| String::new()),
            },
            Fact::Theme => self
                .theme
                .clone()
                .ok_or_else(|| "theme is not available in a headless instance".to_string()),
            Fact::FgProcess => {
                let term = self.term.as_ref().ok_or("the pane has no terminal here")?;
                if self.row()?.domain != "local" {
                    return Err("not available in a remote pane".into());
                }
                Ok(term
                    .foreground_process()
                    .map_or_else(String::new, |p| p.name))
            }
            Fact::ShellPid => {
                let term = self.term.as_ref().ok_or("the pane has no terminal here")?;
                match term.pid() {
                    0 => Err("not available in a remote pane".into()),
                    pid => Ok(pid.to_string()),
                }
            }
            Fact::LastCommand(n) | Fact::LastOutput(n) => {
                let term = self.term.as_ref().ok_or("the pane has no terminal here")?;
                if !term.has_marks() {
                    return Err("needs shell integration (OSC 133 marks)".into());
                }
                let c = term
                    .last_command(*n)
                    .ok_or_else(|| format!("no command #{n} in the history"))?;
                let max = usize::try_from(self.cfg.ctl.capture_max_bytes).unwrap_or(usize::MAX);
                Ok(if matches!(fact, Fact::LastCommand(_)) {
                    c.command
                } else {
                    tail(&c.output, max)
                })
            }
            Fact::Selection => {
                let term = self
                    .term
                    .as_ref()
                    .ok_or("selection is not available here")?;
                Ok(term
                    .selection_text(self.cfg.selection.trim_trailing_whitespace)
                    .unwrap_or_default())
            }
            Fact::Clipboard => self
                .clipboard
                .clone()
                .ok_or_else(|| "the clipboard is not available in a headless instance".to_string()),
            // Answered by `system::system_fact` above.
            Fact::User
            | Fact::Hostname
            | Fact::Home
            | Fact::Tmpdir
            | Fact::Cpus
            | Fact::LocalIp { .. } => Err("unavailable".into()),
        }
    }

    fn counter_get(&self, name: &str) -> Option<i64> {
        COUNTERS
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|m| m.get(&(self.window_key(), name.to_string())).copied())
    }

    fn counter_commit(&mut self, name: &str, next: i64) {
        let key = (self.window_key(), name.to_string());
        COUNTERS
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(key, next);
    }
}

/// The last `max` bytes of `s`, cut on a character boundary.
fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    s[start..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_the_end_on_a_char_boundary() {
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("abc", 10), "abc");
        assert_eq!(tail("aéb", 2), "b");
    }
}
