//! The snippet fill dialog's state (SPEC §10.8.5 step 3): one field per prompt in order of first
//! appearance, remembered values, a live preview and its keys. No GPUI.

use std::collections::HashMap;
use std::sync::Arc;

use mt_snippets::{
    expand, Context, Expanded, Mode, Prompt, PromptKind, RuntimeError, Session, Template, Val,
};

use crate::components::TextEdit;

/// What a key press in the dialog asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillAction {
    Nothing,
    /// Insert; `run` is the final decision (`⌥Enter` forces it).
    Insert {
        run: bool,
    },
    Cancel,
}

#[derive(Debug)]
pub struct FillField {
    pub prompt: Prompt,
    /// `input` and `secret`: the text. `choice`: the filter.
    pub edit: TextEdit,
    /// `choice`: index into the filtered options.
    pub highlight: usize,
}

impl FillField {
    fn filtered(&self) -> Vec<&str> {
        let q = self.edit.text().to_lowercase();
        self.prompt
            .options
            .iter()
            .map(String::as_str)
            .filter(|o| q.is_empty() || o.to_lowercase().contains(&q))
            .collect()
    }
}

pub struct Fill {
    pub snippet_id: String,
    pub title: String,
    pub template: Arc<Template>,
    pub fields: Vec<FillField>,
    pub focus: usize,
    pub session: Session,
    /// The snippet's own `run` unless the gesture overrode it.
    pub run: bool,
    pub broadcast: bool,
    /// Panes the text goes to (captured when the dialog opened).
    pub targets: Vec<u64>,
    /// Last preview failure, shown under the preview.
    pub error: Option<String>,
    /// "Copy expanded": the result goes to the clipboard, not a pane.
    pub copy: bool,
}

pub fn remembered_key(snippet_id: &str, label: &str) -> String {
    format!("{snippet_id}\u{0}{label}")
}

impl Fill {
    #[allow(clippy::too_many_arguments)]
    /// `remembered` holds `(snippet id, label)` → value from earlier fills in this window;
    /// secrets are never in it.
    pub fn new(
        snippet_id: &str,
        title: &str,
        template: Arc<Template>,
        remembered: &HashMap<String, String>,
        ctx: &dyn Context,
        run: bool,
        broadcast: bool,
        targets: Vec<u64>,
    ) -> Self {
        let fields = template
            .analysis
            .prompts
            .iter()
            .map(|p| {
                let initial = match p.kind {
                    PromptKind::Input => remembered
                        .get(&remembered_key(snippet_id, &p.label))
                        .cloned()
                        .or_else(|| p.default.clone())
                        .unwrap_or_default(),
                    PromptKind::Choice => String::new(),
                    PromptKind::Secret => String::new(),
                };
                let mut f = FillField {
                    prompt: p.clone(),
                    edit: TextEdit::new(initial),
                    highlight: 0,
                };
                if p.kind == PromptKind::Choice {
                    if let Some(prev) = remembered.get(&remembered_key(snippet_id, &p.label)) {
                        if let Some(i) = p.options.iter().position(|o| o == prev) {
                            f.highlight = i;
                        }
                    }
                }
                f
            })
            .collect();
        Self {
            snippet_id: snippet_id.to_string(),
            title: title.to_string(),
            template,
            fields,
            focus: 0,
            session: Session::new(ctx),
            run,
            broadcast,
            targets,
            error: None,
            copy: false,
        }
    }

    /// The value the field currently stands for.
    pub fn value(&self, i: usize) -> Val {
        let f = &self.fields[i];
        match f.prompt.kind {
            PromptKind::Input => Val::plain(f.edit.text()),
            PromptKind::Secret => Val::secret(f.edit.text()),
            PromptKind::Choice => {
                let shown = f.filtered();
                let pick = shown
                    .get(f.highlight)
                    .copied()
                    .or_else(|| f.prompt.options.get(f.highlight).map(String::as_str))
                    .unwrap_or("");
                Val::plain(pick)
            }
        }
    }

    /// Filtered options of a `choice` field with the highlighted one's index.
    pub fn options(&self, i: usize) -> (Vec<&str>, usize) {
        let f = &self.fields[i];
        let shown = f.filtered();
        let hl = f.highlight.min(shown.len().saturating_sub(1));
        (shown, hl)
    }

    /// Pushes every field into the session (call before previewing or inserting).
    pub fn apply_values(&mut self) {
        for i in 0..self.fields.len() {
            let v = self.value(i);
            let label = self.fields[i].prompt.label.clone();
            self.session.set_prompt(&label, v);
        }
    }

    /// The live preview: exactly what an insert would send, with secrets masked.
    pub fn preview(&mut self, ctx: &mut dyn Context) -> Result<Expanded, RuntimeError> {
        self.apply_values();
        let r = expand(&self.template.clone(), ctx, &mut self.session, Mode::Masked);
        self.error = r.as_ref().err().map(ToString::to_string);
        r
    }

    /// The values to remember for next time (never secrets).
    pub fn to_remember(&self) -> Vec<(String, String)> {
        self.fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.prompt.kind != PromptKind::Secret)
            .map(|(i, f)| {
                (
                    remembered_key(&self.snippet_id, &f.prompt.label),
                    self.value(i).expose().to_string(),
                )
            })
            .collect()
    }

    pub fn focus_next(&mut self, back: bool) {
        let n = self.fields.len();
        if n > 0 {
            self.focus = if back {
                (self.focus + n - 1) % n
            } else {
                (self.focus + 1) % n
            };
        }
    }

    pub fn focused(&mut self) -> Option<&mut FillField> {
        self.fields.get_mut(self.focus)
    }

    pub fn type_text(&mut self, text: &str) {
        if let Some(f) = self.focused() {
            f.edit.insert(text);
            f.highlight = 0;
        }
    }

    /// Key presses other than plain typing. `key` is the GPUI key name.
    pub fn key(&mut self, key: &str, alt: bool, shift: bool) -> FillAction {
        match key {
            "escape" => return FillAction::Cancel,
            "enter" => {
                return FillAction::Insert {
                    run: alt || self.run,
                }
            }
            "tab" => self.focus_next(shift),
            "up" | "down" => {
                let down = key == "down";
                if let Some(f) = self.fields.get(self.focus) {
                    if f.prompt.kind == PromptKind::Choice {
                        let (shown, hl) = self.options(self.focus);
                        let n = shown.len();
                        let next = if n == 0 {
                            0
                        } else if down {
                            (hl + 1).min(n - 1)
                        } else {
                            hl.saturating_sub(1)
                        };
                        self.fields[self.focus].highlight = next;
                    } else if down {
                        self.focus_next(false);
                    } else {
                        self.focus_next(true);
                    }
                }
            }
            "backspace" => {
                if let Some(f) = self.focused() {
                    f.edit.backspace();
                    f.highlight = 0;
                }
            }
            "delete" => {
                if let Some(f) = self.focused() {
                    f.edit.delete_forward();
                }
            }
            "left" => {
                if let Some(f) = self.focused() {
                    f.edit.left(shift);
                }
            }
            "right" => {
                if let Some(f) = self.focused() {
                    f.edit.right(shift);
                }
            }
            "home" => {
                if let Some(f) = self.focused() {
                    f.edit.home(shift);
                }
            }
            "end" => {
                if let Some(f) = self.focused() {
                    f.edit.end(shift);
                }
            }
            _ => {}
        }
        FillAction::Nothing
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mt_snippets::chrono::{DateTime, FixedOffset};
    use mt_snippets::Fact;

    struct Ctx;
    impl Context for Ctx {
        fn now(&self) -> DateTime<FixedOffset> {
            DateTime::from_timestamp(1_700_000_000, 0)
                .unwrap()
                .fixed_offset()
        }
        fn fill_random(&mut self, buf: &mut [u8]) {
            buf.fill(1);
        }
        fn env(&self, _: &str) -> Option<String> {
            None
        }
        fn fact(&mut self, _: &Fact) -> Result<String, String> {
            Err("none".into())
        }
        fn counter_get(&self, _: &str) -> Option<i64> {
            None
        }
    }

    const BODY: &str = "git checkout -b {{ b := input \"Branch\" | kebab }} && curl -H {{ secret \"Token\" }} {{ choice \"Env\" \"dev\" \"staging\" \"prod\" }} {{ input \"Branch\" }} {{ uuid }}";

    fn fill(remembered: &HashMap<String, String>) -> Fill {
        Fill::new(
            "s",
            "S",
            Template::cached(BODY),
            remembered,
            &Ctx,
            false,
            false,
            vec![1],
        )
    }

    #[test]
    fn fields_follow_first_appearance_and_share_labels() {
        let f = fill(&HashMap::new());
        let labels: Vec<_> = f.fields.iter().map(|f| f.prompt.label.as_str()).collect();
        assert_eq!(labels, ["Branch", "Token", "Env"]);
    }

    #[test]
    fn remembered_values_prefill_but_never_for_secrets() {
        let mut mem = HashMap::new();
        mem.insert(remembered_key("s", "Branch"), "Fix It".to_string());
        mem.insert(remembered_key("s", "Token"), "leaked".to_string());
        mem.insert(remembered_key("s", "Env"), "prod".to_string());
        let f = fill(&mem);
        assert_eq!(f.value(0).expose(), "Fix It");
        assert_eq!(f.value(1).expose(), "", "secrets start empty");
        assert_eq!(f.value(2).expose(), "prod");
        let kept = f.to_remember();
        assert!(kept
            .iter()
            .all(|(k, v)| !k.contains("Token") && v != "leaked"));
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn preview_is_masked_and_equals_the_insert_text() {
        let mut f = fill(&HashMap::new());
        f.type_text("Fix The Bug");
        f.focus_next(false);
        f.type_text("hunter2");
        let mut ctx = Ctx;
        let p = f.preview(&mut ctx).unwrap();
        assert!(p.text.contains("fix-the-bug") && p.text.contains("••••"));
        assert!(!p.text.contains("hunter2"));
        // The insert expands the same session in Insert mode: same uuid, real secret.
        f.apply_values();
        let real = expand(&f.template.clone(), &mut ctx, &mut f.session, Mode::Insert).unwrap();
        assert_eq!(real.text, p.text.replace("••••", "hunter2"));
    }

    #[test]
    fn keys() {
        let mut f = fill(&HashMap::new());
        assert_eq!(f.key("tab", false, false), FillAction::Nothing);
        assert_eq!(f.focus, 1);
        f.key("tab", false, true);
        f.key("tab", false, true);
        assert_eq!(f.focus, 2, "shift-tab wraps backwards");
        assert_eq!(
            f.key("enter", false, false),
            FillAction::Insert { run: false }
        );
        assert_eq!(
            f.key("enter", true, false),
            FillAction::Insert { run: true }
        );
        f.run = true;
        assert_eq!(
            f.key("enter", false, false),
            FillAction::Insert { run: true }
        );
        assert_eq!(f.key("escape", false, false), FillAction::Cancel);
    }

    #[test]
    fn choice_is_preselected_filterable_and_navigable() {
        let mut f = fill(&HashMap::new());
        f.focus = 2;
        assert_eq!(f.value(2).expose(), "dev");
        f.key("down", false, false);
        assert_eq!(f.value(2).expose(), "staging");
        f.key("down", false, false);
        f.key("down", false, false);
        assert_eq!(f.value(2).expose(), "prod", "stops at the end");
        f.key("up", false, false);
        assert_eq!(f.value(2).expose(), "staging");
        f.type_text("pr");
        assert_eq!(f.options(2).0, ["prod"]);
        assert_eq!(f.value(2).expose(), "prod");
        f.key("backspace", false, false);
        f.key("backspace", false, false);
        assert_eq!(f.options(2).0.len(), 3);
    }

    #[test]
    fn up_down_move_between_text_fields() {
        let mut f = fill(&HashMap::new());
        f.key("down", false, false);
        assert_eq!(f.focus, 1);
        f.key("up", false, false);
        assert_eq!(f.focus, 0);
    }

    #[test]
    fn editing_keys_edit_the_focused_field() {
        let mut f = fill(&HashMap::new());
        f.type_text("abc");
        f.key("left", false, false);
        f.key("backspace", false, false);
        assert_eq!(f.value(0).expose(), "ac");
        f.key("home", false, false);
        f.type_text("x");
        assert_eq!(f.value(0).expose(), "xac");
    }
}
