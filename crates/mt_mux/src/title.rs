//! Tab title formatting and close-confirmation policy.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmClose {
    Always,
    Process,
    Never,
}

/// Whether closing a tab/window needs a confirmation dialog.
pub fn should_confirm_close(policy: ConfirmClose, has_foreground_process: bool) -> bool {
    match policy {
        ConfirmClose::Always => true,
        ConfirmClose::Process => has_foreground_process,
        ConfirmClose::Never => false,
    }
}

#[derive(Debug, Clone, Default)]
pub struct TitleParts<'a> {
    /// Set by RenameTab; wins over everything and is not run through the format.
    pub title_override: Option<&'a str>,
    /// Set by a trigger rule; wins over the pane and process titles, loses to a rename.
    pub trigger_title: Option<&'a str>,
    /// OSC 0/2 title of the pane.
    pub pane_title: Option<&'a str>,
    /// Foreground process name.
    pub process: Option<&'a str>,
    pub cwd: Option<&'a Path>,
    /// Zero-based tab position.
    pub index: usize,
    pub show_index: bool,
}

/// `title_override ?? pane title ?? process`, then `title_format` placeholders
/// `{title} {process} {cwd_basename} {index}` (SPEC §5, §8.3). Unknown placeholders stay literal.
pub fn format_title(parts: &TitleParts<'_>, format: &str) -> String {
    let base = if let Some(t) = parts.title_override.filter(|t| !t.is_empty()) {
        t.to_string()
    } else if let Some(t) = parts.trigger_title.filter(|t| !t.is_empty()) {
        t.to_string()
    } else {
        let title = parts
            .pane_title
            .filter(|t| !t.is_empty())
            .or(parts.process)
            .unwrap_or("");
        let process = parts.process.unwrap_or("");
        let cwd_basename = parts
            .cwd
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        format
            .replace("{title}", title)
            .replace("{process}", process)
            .replace("{cwd_basename}", &cwd_basename)
            .replace("{index}", &(parts.index + 1).to_string())
    };
    if parts.show_index {
        format!("{} {}", parts.index + 1, base)
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts() -> TitleParts<'static> {
        TitleParts {
            pane_title: Some("vim main.rs"),
            process: Some("vim"),
            cwd: Some(Path::new("/home/me/proj")),
            index: 2,
            ..Default::default()
        }
    }

    #[test]
    fn default_format_uses_pane_title() {
        assert_eq!(format_title(&parts(), "{title}"), "vim main.rs");
    }

    #[test]
    fn falls_back_to_process() {
        let mut p = parts();
        p.pane_title = None;
        assert_eq!(format_title(&p, "{title}"), "vim");
        p.pane_title = Some("");
        assert_eq!(format_title(&p, "{title}"), "vim");
    }

    #[test]
    fn all_placeholders() {
        assert_eq!(
            format_title(&parts(), "{index}:{process} {cwd_basename}"),
            "3:vim proj"
        );
    }

    #[test]
    fn trigger_title_sits_between_rename_and_osc_title() {
        let mut p = parts();
        p.trigger_title = Some("BUILD FAILED");
        assert_eq!(format_title(&p, "{index}:{title}"), "BUILD FAILED");
        p.title_override = Some("api");
        assert_eq!(format_title(&p, "{title}"), "api", "a manual rename wins");
        p.title_override = None;
        p.trigger_title = Some("");
        assert_eq!(format_title(&p, "{title}"), "vim main.rs");
        p.trigger_title = None;
        assert_eq!(format_title(&p, "{title}"), "vim main.rs");
    }

    #[test]
    fn override_wins_and_skips_format() {
        let mut p = parts();
        p.title_override = Some("api");
        assert_eq!(format_title(&p, "{index}:{title}"), "api");
    }

    #[test]
    fn unknown_placeholder_is_literal() {
        assert_eq!(format_title(&parts(), "{nope}"), "{nope}");
    }

    #[test]
    fn show_index_prefixes() {
        let mut p = parts();
        p.show_index = true;
        assert_eq!(format_title(&p, "{title}"), "3 vim main.rs");
    }

    #[test]
    fn confirm_close_decisions() {
        use ConfirmClose::*;
        assert!(should_confirm_close(Always, false));
        assert!(should_confirm_close(Process, true));
        assert!(!should_confirm_close(Process, false));
        assert!(!should_confirm_close(Never, true));
    }
}
