//! Turns a firing into action *intents*: pure values with every template already expanded and
//! sanitised. The UI layer applies them; nothing here touches the PTY or the window (§10.11.4).

use crate::matcher::Hit;
use crate::model::*;
use crate::template::{expand, sanitize_with};

/// The facts of one firing.
#[derive(Debug, Clone, Default)]
pub struct FireCtx {
    pub rule_name: String,
    pub pane: String,
    pub title: String,
    pub cwd: String,
    pub domain: String,
    /// Text rules: the matched logical line.
    pub line: Option<String>,
    pub hit: Option<Hit>,
    /// Event rules: the event name and exit code, when it has one.
    pub event: Option<(String, Option<i64>)>,
}

impl FireCtx {
    fn lookup(&self, name: &str) -> Option<String> {
        match name {
            "rule" => Some(self.rule_name.clone()),
            "pane" => Some(self.pane.clone()),
            "title" => Some(self.title.clone()),
            "cwd" => Some(self.cwd.clone()),
            "domain" => Some(self.domain.clone()),
            "line" => self.line.clone(),
            "match" => self.hit.as_ref().map(|h| h.whole().to_string()),
            "event" => self.event.as_ref().map(|(e, _)| e.clone()),
            "exit" => self
                .event
                .as_ref()
                .and_then(|(_, c)| c.map(|c| c.to_string())),
            n => match n.parse::<usize>() {
                Ok(i) => self.hit.as_ref()?.captures.get(i)?.clone(),
                Err(_) => self.hit.as_ref()?.named(n).map(str::to_string),
            },
        }
    }

    fn expand(&self, template: &str, max: usize) -> String {
        sanitize_with(&expand(template, |n| self.lookup(n)), max)
    }

    /// The capture text selected by a clipboard `source`.
    fn capture(&self, source: &str) -> Option<String> {
        if source == "match" {
            self.lookup("match")
        } else {
            self.lookup(source)
        }
    }
}

/// What the UI should do. Parameters are final; text is sanitised.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    Highlight {
        /// Character offsets into the line; `None` = the whole logical line.
        span: Option<(usize, usize)>,
        /// The text the highlight was drawn for (the match, or the whole line); it is drawn only
        /// while the screen still shows exactly this there.
        expect: String,
        color: String,
        style: Style,
        ttl_s: u32,
    },
    Notify {
        level: Level,
        title: String,
        message: String,
        desktop: bool,
    },
    Status {
        state: State,
        progress: Option<u8>,
        text: String,
        ttl_s: u32,
    },
    TabColor {
        color: String,
        ttl_s: u32,
        clear_on_focus: bool,
    },
    Bell,
    Clipboard {
        text: String,
    },
    RecordingStart {
        stop_after_s: u32,
    },
    RecordingStop,
    Marker {
        label: String,
        color: String,
    },
    Counter {
        reset_on: ResetOn,
    },
    Title {
        text: String,
        ttl_s: u32,
    },
    Findings {
        label: String,
        line: String,
        /// The matched characters of the line (text rules), for the jump's flash.
        span: Option<(usize, usize)>,
    },
    Snapshot {
        before: u32,
        after: u32,
    },
    Screenshot {
        scale: f32,
        chrome: ShotChrome,
    },
}

impl Intent {
    pub fn kind(&self) -> &'static str {
        match self {
            Intent::Highlight { .. } => "highlight",
            Intent::Notify { .. } => "notify",
            Intent::Status { .. } => "status",
            Intent::TabColor { .. } => "tab_color",
            Intent::Bell => "bell",
            Intent::Clipboard { .. } => "clipboard",
            Intent::RecordingStart { .. } | Intent::RecordingStop => "recording",
            Intent::Marker { .. } => "marker",
            Intent::Counter { .. } => "counter",
            Intent::Title { .. } => "title",
            Intent::Findings { .. } => "findings",
            Intent::Snapshot { .. } => "snapshot",
            Intent::Screenshot { .. } => "screenshot",
        }
    }

    pub fn gate(&self) -> Option<Gate> {
        match self {
            Intent::Clipboard { .. } => Some(Gate::Clipboard),
            Intent::Snapshot { .. } | Intent::Screenshot { .. } => Some(Gate::Capture),
            _ => None,
        }
    }

    /// One-line description for the **Test** dry run and the `trigger.fired` event.
    pub fn describe(&self) -> String {
        match self {
            Intent::Highlight { span, color, .. } => format!(
                "highlight {} in {color}",
                if span.is_some() {
                    "the match"
                } else {
                    "the line"
                }
            ),
            Intent::Notify {
                level,
                title,
                message,
                ..
            } => {
                format!("notify ({level:?}) “{title}”: {message}")
            }
            Intent::Status {
                state,
                progress,
                text,
                ..
            } => {
                format!(
                    "status {state:?}{} {text}",
                    progress.map_or(String::new(), |p| format!(" {p}%"))
                )
            }
            Intent::TabColor { color, .. } => format!("tab color {color}"),
            Intent::Bell => "bell".to_string(),
            Intent::Clipboard { text } => {
                format!("copy {} characters to the clipboard", text.chars().count())
            }
            Intent::RecordingStart { stop_after_s } => {
                format!("start recording (stop after {stop_after_s} s)")
            }
            Intent::RecordingStop => "stop trigger recording".to_string(),
            Intent::Marker { label, .. } => format!("marker “{label}”"),
            Intent::Counter { reset_on } => format!("count (reset on {reset_on:?})"),
            Intent::Title { text, .. } => format!("set tab title “{text}”"),
            Intent::Findings { label, .. } => format!("add finding “{label}”"),
            Intent::Snapshot { before, after } => {
                format!("write snapshot (-{before}/+{after} lines)")
            }
            Intent::Screenshot { scale, .. } => format!("write screenshot ×{scale}"),
        }
    }
}

/// Plans the actions of `rule` for one firing, in the order written. Actions that cannot apply
/// (a clipboard source the event has no text for) are left out.
pub fn plan(rule: &Rule, ctx: &FireCtx) -> Vec<Intent> {
    let span = ctx.hit.as_ref().map(|h| (h.start, h.end));
    let line_default = || {
        if ctx.line.is_some() {
            "{line}".to_string()
        } else {
            ctx.rule_name.clone()
        }
    };
    rule.actions
        .iter()
        .filter_map(|a| {
            Some(match a {
                Action::Highlight {
                    scope,
                    color,
                    style,
                    ttl_s,
                } => Intent::Highlight {
                    span: if *scope == Scope::Match { span } else { None },
                    expect: if *scope == Scope::Match {
                        ctx.hit.as_ref().map(|h| h.whole().to_string())
                    } else {
                        ctx.line.clone()
                    }
                    .unwrap_or_default(),
                    color: color.clone(),
                    style: *style,
                    ttl_s: *ttl_s,
                },
                Action::Notify {
                    level,
                    title,
                    message,
                    desktop,
                } => {
                    let title = if title.is_empty() {
                        ctx.rule_name.clone()
                    } else {
                        ctx.expand(title, 120)
                    };
                    let message = if message.is_empty() {
                        line_default()
                    } else {
                        message.clone()
                    };
                    Intent::Notify {
                        level: *level,
                        title,
                        message: ctx.expand(&message, 120),
                        desktop: *desktop,
                    }
                }
                Action::Status {
                    state,
                    progress,
                    text,
                    ttl_s,
                } => Intent::Status {
                    state: *state,
                    progress: match progress {
                        None => None,
                        Some(Progress::Value(v)) => Some((*v).clamp(0, 100) as u8),
                        Some(Progress::Template(t)) => ctx
                            .expand(t, 20)
                            .trim()
                            .parse::<i64>()
                            .ok()
                            .map(|v| v.clamp(0, 100) as u8),
                    },
                    text: ctx.expand(text, 120),
                    ttl_s: *ttl_s,
                },
                Action::TabColor {
                    color,
                    ttl_s,
                    clear_on_focus,
                } => Intent::TabColor {
                    color: color.clone(),
                    ttl_s: *ttl_s,
                    clear_on_focus: *clear_on_focus,
                },
                Action::Bell {} => Intent::Bell,
                Action::Clipboard { source } => Intent::Clipboard {
                    text: sanitize_with(&ctx.capture(source)?, 4096),
                },
                Action::Recording {
                    op: RecOp::Start,
                    stop_after_s,
                } => Intent::RecordingStart {
                    stop_after_s: *stop_after_s,
                },
                Action::Recording {
                    op: RecOp::Stop, ..
                } => Intent::RecordingStop,
                Action::Marker { label, color } => Intent::Marker {
                    label: ctx.expand(if label.is_empty() { "{rule}" } else { label }, 80),
                    color: color.clone(),
                },
                Action::Counter { reset_on } => Intent::Counter {
                    reset_on: *reset_on,
                },
                Action::Title { template, ttl_s } => Intent::Title {
                    text: ctx.expand(template, 60),
                    ttl_s: *ttl_s,
                },
                Action::Findings { label } => Intent::Findings {
                    label: ctx.expand(if label.is_empty() { "{line}" } else { label }, 120),
                    line: sanitize_with(ctx.line.as_deref().unwrap_or(""), 200),
                    span,
                },
                Action::Snapshot { before, after } => Intent::Snapshot {
                    before: *before,
                    after: *after,
                },
                Action::Screenshot { scale, chrome } => Intent::Screenshot {
                    scale: *scale,
                    chrome: *chrome,
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::Matcher;

    fn rule(actions: Vec<Action>) -> Rule {
        let mut r = Rule::new_text("Build failed", r"error(\[E\d+\])?: (?P<msg>.+)");
        r.actions = actions;
        r
    }

    fn ctx_for(r: &Rule, line: &str) -> FireCtx {
        let hit = Matcher::new(std::slice::from_ref(r)).scan(line).pop();
        FireCtx {
            rule_name: r.name.clone(),
            pane: "3".into(),
            title: "zsh".into(),
            cwd: "/tmp".into(),
            domain: "local".into(),
            line: Some(line.into()),
            hit,
            event: None,
        }
    }

    #[test]
    fn notify_expands_captures_and_sanitises() {
        let r = rule(vec![Action::Notify {
            level: Level::Error,
            title: String::new(),
            message: "{msg} in {cwd} [{1}]".into(),
            desktop: true,
        }]);
        let ctx = ctx_for(&r, "error[E0308]: bad\u{1b}[0m   types");
        let p = plan(&r, &ctx);
        assert_eq!(
            p,
            vec![Intent::Notify {
                level: Level::Error,
                title: "Build failed".into(),
                message: "bad[0m types in /tmp [[E0308]]".into(),
                desktop: true,
            }]
        );
    }

    #[test]
    fn highlight_scope_decides_the_span() {
        let hl = |scope| Action::Highlight {
            scope,
            color: "red".into(),
            style: Style::Background,
            ttl_s: 0,
        };
        let r = rule(vec![hl(Scope::Match), hl(Scope::Line)]);
        let p = plan(&r, &ctx_for(&r, "x error: boom"));
        assert!(matches!(
            p[0],
            Intent::Highlight {
                span: Some((2, 13)),
                ..
            }
        ));
        assert!(matches!(p[1], Intent::Highlight { span: None, .. }));
    }

    #[test]
    fn status_progress_forms() {
        let st = |p| Action::Status {
            state: State::Working,
            progress: Some(p),
            text: String::new(),
            ttl_s: 0,
        };
        let mut r = Rule::new_text("p", r"(?P<pct>\d+)%");
        r.actions = vec![
            st(Progress::Value(250)),
            st(Progress::Value(-3)),
            st(Progress::Template("{pct}".into())),
            st(Progress::Template("{match}".into())),
            st(Progress::Template("{line}".into())),
            st(Progress::Template("999".into())),
        ];
        let prog: Vec<_> = plan(&r, &ctx_for(&r, "done 42%"))
            .into_iter()
            .map(|i| match i {
                Intent::Status { progress, .. } => progress,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            prog,
            vec![Some(100), Some(0), Some(42), None, None, Some(100)]
        );
    }

    #[test]
    fn clipboard_sources_and_missing_text() {
        let clip = |s: &str| Action::Clipboard { source: s.into() };
        let r = rule(vec![clip("match"), clip("msg"), clip("1"), clip("9")]);
        let p = plan(&r, &ctx_for(&r, "error: a\nb"));
        let texts: Vec<_> = p
            .iter()
            .map(|i| match i {
                Intent::Clipboard { text } => text.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(texts, vec!["error: a", "a"]);
        // An event firing has no capture text, so nothing is copied.
        let ctx = FireCtx {
            event: Some(("bell".into(), None)),
            ..FireCtx::default()
        };
        assert!(plan(&r, &ctx).is_empty());
    }

    #[test]
    fn clipboard_is_one_line_and_capped() {
        let r = rule(vec![Action::Clipboard {
            source: "match".into(),
        }]);
        let long = format!("error: {}", "x ".repeat(5000));
        let Intent::Clipboard { text } = &plan(&r, &ctx_for(&r, &long))[0] else {
            panic!()
        };
        assert!(text.chars().count() <= 4096 && !text.contains('\n'));
    }

    #[test]
    fn event_placeholders_and_defaults() {
        let mut r = Rule::new_text("Done", "x");
        r.when = When::Event {
            event: EventKind::CommandFinished,
            exit: ExitFilter::Any,
        };
        r.actions = vec![
            Action::Notify {
                level: Level::Info,
                title: String::new(),
                message: "{title} exited with {exit} ({event})".into(),
                desktop: false,
            },
            Action::Marker {
                label: String::new(),
                color: String::new(),
            },
            Action::Findings {
                label: String::new(),
            },
        ];
        let ctx = FireCtx {
            rule_name: "Done".into(),
            title: "make".into(),
            event: Some(("command_finished".into(), Some(2))),
            ..FireCtx::default()
        };
        let p = plan(&r, &ctx);
        assert!(
            matches!(&p[0], Intent::Notify { message, .. } if message == "make exited with 2 (command_finished)")
        );
        assert!(matches!(&p[1], Intent::Marker { label, .. } if label == "Done"));
        assert!(matches!(&p[2], Intent::Findings { line, .. } if line.is_empty()));
    }

    #[test]
    fn title_and_marker_are_capped() {
        let r = rule(vec![
            Action::Title {
                template: "{line}".into(),
                ttl_s: 5,
            },
            Action::Marker {
                label: "{line}".into(),
                color: "red".into(),
            },
        ]);
        let long = format!("error: {}", "w".repeat(300));
        let p = plan(&r, &ctx_for(&r, &long));
        assert!(matches!(&p[0], Intent::Title { text, .. } if text.chars().count() <= 60));
        assert!(matches!(&p[1], Intent::Marker { label, .. } if label.chars().count() <= 80));
    }

    #[test]
    fn gates_and_descriptions() {
        let r = rule(vec![
            Action::Clipboard {
                source: "match".into(),
            },
            Action::Snapshot {
                before: 1,
                after: 2,
            },
            Action::Screenshot {
                scale: 1.0,
                chrome: ShotChrome::None,
            },
            Action::Bell {},
        ]);
        let p = plan(&r, &ctx_for(&r, "error: z"));
        let gates: Vec<_> = p.iter().map(Intent::gate).collect();
        assert_eq!(
            gates,
            vec![
                Some(Gate::Clipboard),
                Some(Gate::Capture),
                Some(Gate::Capture),
                None
            ]
        );
        assert!(p.iter().all(|i| !i.describe().is_empty()));
    }
}

/// One firing of a dry run: the line (or event) and the intents that **would** run.
#[derive(Debug, Clone, PartialEq)]
pub struct DryFiring {
    pub input: String,
    pub intents: Vec<Intent>,
}

/// The **Test** button (§10.11.5): evaluates a text `rule` against `lines` and lists what would
/// run, with expanded text. Runs nothing: no limiter, no decoration, no file. Event rules
/// produce no firings here; use [`dry_run_event`].
pub fn dry_run_text(rule: &Rule, lines: &[String], base: &FireCtx) -> Vec<DryFiring> {
    let matcher = crate::Matcher::new(std::slice::from_ref(&Rule {
        enabled: true,
        ..rule.clone()
    }));
    lines
        .iter()
        .filter_map(|line| {
            let hit = matcher.scan(line).pop()?;
            let ctx = FireCtx {
                line: Some(line.clone()),
                hit: Some(hit),
                rule_name: rule.name.clone(),
                ..base.clone()
            };
            Some(DryFiring {
                input: line.clone(),
                intents: plan(rule, &ctx),
            })
        })
        .collect()
}

/// Sample firings of an event rule: one per exit code the filter accepts (0 and 1; the code
/// itself for `exit = N`), so `{exit}` expands to something real.
pub fn dry_run_event(rule: &Rule, base: &FireCtx) -> Vec<DryFiring> {
    let When::Event { event, exit } = &rule.when else {
        return Vec::new();
    };
    let samples: Vec<Option<i64>> = if event.has_exit() {
        match exit {
            ExitFilter::Any => vec![Some(0), Some(1)],
            ExitFilter::Success => vec![Some(0)],
            ExitFilter::Failure => vec![Some(1)],
            ExitFilter::Code(c) => vec![Some(*c)],
        }
    } else {
        vec![None]
    };
    samples
        .into_iter()
        .map(|code| {
            let ctx = FireCtx {
                rule_name: rule.name.clone(),
                event: Some((event.name().to_string(), code)),
                line: None,
                hit: None,
                ..base.clone()
            };
            DryFiring {
                input: match code {
                    Some(c) => format!("{} (exit {c})", event.name()),
                    None => event.name().to_string(),
                },
                intents: plan(rule, &ctx),
            }
        })
        .collect()
}

#[cfg(test)]
mod dry_tests {
    use super::*;

    #[test]
    fn text_dry_run_lists_would_be_intents_per_matching_line() {
        let mut r = Rule::new_text("Build failed", r"error: (?P<m>.+)");
        r.actions = vec![
            Action::Notify {
                level: Level::Error,
                title: String::new(),
                message: "{m}".into(),
                desktop: false,
            },
            Action::Bell {},
        ];
        let lines: Vec<String> = ["ok", "error: one", "fine", "error: two"]
            .map(String::from)
            .to_vec();
        let out = dry_run_text(&r, &lines, &FireCtx::default());
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].input, "error: one");
        assert!(matches!(&out[1].intents[0], Intent::Notify { message, .. } if message == "two"));
        // A disabled rule can still be tested.
        r.enabled = false;
        assert_eq!(dry_run_text(&r, &lines, &FireCtx::default()).len(), 2);
    }

    #[test]
    fn event_dry_run_samples_exit_codes() {
        let mut r = Rule::new_text("Done", "x");
        r.when = When::Event {
            event: EventKind::CommandFinished,
            exit: ExitFilter::Any,
        };
        r.actions = vec![Action::Notify {
            level: Level::Info,
            title: String::new(),
            message: "exited {exit}".into(),
            desktop: false,
        }];
        let out = dry_run_event(&r, &FireCtx::default());
        let msgs: Vec<_> = out
            .iter()
            .map(|d| match &d.intents[0] {
                Intent::Notify { message, .. } => message.clone(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(msgs, ["exited 0", "exited 1"]);
        r.when = When::Event {
            event: EventKind::Bell,
            exit: ExitFilter::Any,
        };
        assert_eq!(dry_run_event(&r, &FireCtx::default()).len(), 1);
        assert!(dry_run_event(&Rule::new_text("t", "x"), &FireCtx::default()).is_empty());
    }
}
