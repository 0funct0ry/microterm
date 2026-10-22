//! The snippet editor's state (SPEC §10.8.6 "Editor"): fields, dirty check, save gating and the
//! snippet it would write. No GPUI.

use mt_config::keys::Keystroke;
use mt_snippets::model::{slugify, unique_id, MAX_BODY_BYTES};
use mt_snippets::store::Base;
use mt_snippets::{Snippet, SnippetSet, Template};

use crate::components::{TextAreaEdit, TextEdit};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EditorField {
    Title,
    Folder,
    Tags,
    Description,
    Body,
    Run,
    Key,
}

const ORDER: [EditorField; 7] = [
    EditorField::Title,
    EditorField::Folder,
    EditorField::Tags,
    EditorField::Description,
    EditorField::Body,
    EditorField::Run,
    EditorField::Key,
];

pub struct Editor {
    /// The id being edited; `None` for a new snippet.
    pub id: Option<String>,
    /// What the file looked like when the editor opened (per-entry staleness).
    pub base: Base,
    /// The snippet as loaded (default for a new one); the dirty check compares against it.
    pub original: Snippet,
    pub title: TextEdit,
    pub folder: TextEdit,
    pub tags: TextEdit,
    pub description: TextEdit,
    pub key: TextEdit,
    pub body: TextAreaEdit,
    pub run: bool,
    pub field: EditorField,
    /// Esc / back asked to discard unsaved changes.
    pub confirm_discard: bool,
    /// A save found the entry changed on disk (SPEC §10.8.2): offer "Reload and re-apply".
    pub stale: bool,
    /// Last save failure, shown in the footer.
    pub error: Option<String>,
}

fn split_tags(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .collect()
}

impl Editor {
    pub fn open(snippet: Snippet, id: Option<String>, base: Base) -> Self {
        let field = if id.is_some() {
            EditorField::Body
        } else {
            EditorField::Title
        };
        Self {
            id,
            base,
            title: TextEdit::new(snippet.title.clone()),
            folder: TextEdit::new(snippet.folder.clone()),
            tags: TextEdit::new(snippet.tags.join(", ")),
            description: TextEdit::new(snippet.description.clone()),
            key: TextEdit::new(snippet.keys.clone()),
            body: TextAreaEdit::new(snippet.body.clone()),
            run: snippet.run,
            field,
            confirm_discard: false,
            stale: false,
            error: None,
            original: snippet,
        }
    }

    /// The text edit of the focused single-line field.
    pub fn line_edit(&mut self) -> Option<&mut TextEdit> {
        match self.field {
            EditorField::Title => Some(&mut self.title),
            EditorField::Folder => Some(&mut self.folder),
            EditorField::Tags => Some(&mut self.tags),
            EditorField::Description => Some(&mut self.description),
            EditorField::Key => Some(&mut self.key),
            EditorField::Body | EditorField::Run => None,
        }
    }

    pub fn focus_next(&mut self, back: bool) {
        let i = ORDER.iter().position(|f| *f == self.field).unwrap_or(0);
        let n = ORDER.len();
        self.field = ORDER[if back { (i + n - 1) % n } else { (i + 1) % n }];
    }

    /// The snippet the fields describe. A new snippet derives its id from the title and avoids
    /// ids already in `set`; an edited one keeps its id.
    pub fn to_snippet(&self, set: &SnippetSet) -> Snippet {
        let title = self.title.text().trim().to_string();
        let id = self
            .id
            .clone()
            .unwrap_or_else(|| unique_id(&slugify(&title), &|c| set.get(c).is_some()));
        Snippet {
            id,
            explicit_id: true,
            title,
            body: self.body.text().to_string(),
            folder: self.folder.text().trim().to_string(),
            tags: split_tags(self.tags.text()),
            description: self.description.text().trim().to_string(),
            run: self.run,
            keys: self.key.text().trim().to_string(),
        }
    }

    /// Unsaved changes compared with what was opened.
    pub fn dirty(&self) -> bool {
        let o = &self.original;
        self.title.text().trim() != o.title
            || self.body.text() != o.body
            || self.folder.text().trim() != o.folder
            || split_tags(self.tags.text()) != o.tags
            || self.description.text().trim() != o.description
            || self.key.text().trim() != o.keys
            || self.run != o.run
    }

    /// A problem with the key field (§9.1 syntax), if any.
    pub fn key_error(&self) -> Option<String> {
        let k = self.key.text().trim();
        if k.is_empty() {
            return None;
        }
        k.parse::<Keystroke>().err().map(|e| e.to_string())
    }

    /// Template diagnostics for the body as `(line, col, message)`; empty with templates off.
    pub fn diagnostics(&self, templates_on: bool) -> Vec<(usize, usize, String)> {
        if !templates_on {
            return Vec::new();
        }
        let t = Template::compile(self.body.text());
        t.diagnostics()
            .into_iter()
            .map(|d| {
                let (l, c) = d.line_col(self.body.text());
                (l, c, d.message)
            })
            .collect()
    }

    /// Why Save is disabled right now (SPEC §10.8.6), or `None` when it is allowed.
    pub fn save_blocker(&self, templates_on: bool) -> Option<String> {
        let title = self.title.text().trim();
        if title.is_empty() {
            return Some("The title is required".into());
        }
        if self.body.text().is_empty() {
            return Some("The body is required".into());
        }
        if self.body.text().len() > MAX_BODY_BYTES {
            return Some(format!("The body is longer than {MAX_BODY_BYTES} bytes"));
        }
        if let Some(e) = self.key_error() {
            return Some(format!("Key: {e}"));
        }
        if let Some((l, c, m)) = self.diagnostics(templates_on).into_iter().next() {
            return Some(format!("Template error at {l}:{c}: {m}"));
        }
        None
    }

    /// A warning that does not block saving: `{{cursor}}` needs a single-line body.
    pub fn cursor_warning(&self, templates_on: bool) -> Option<&'static str> {
        if !templates_on || !self.body.text().contains('\n') {
            return None;
        }
        Template::cached(self.body.text())
            .analysis
            .has_cursor
            .then_some("`cursor` is ignored in a multi-line body")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(body: &str) -> Editor {
        Editor::open(
            Snippet {
                id: "a".into(),
                explicit_id: true,
                title: "A".into(),
                body: body.into(),
                ..Default::default()
            },
            Some("a".into()),
            Base::of("", None),
        )
    }

    fn blank() -> Editor {
        Editor::open(Snippet::default(), None, Base::of("", None))
    }

    #[test]
    fn dirty_tracks_every_field() {
        let mut e = editor("echo");
        assert!(!e.dirty());
        e.run = true;
        assert!(e.dirty());
        e.run = false;
        assert!(!e.dirty());
        e.tags.insert("x, y");
        assert!(e.dirty());
        e.tags = TextEdit::default();
        e.body.insert(" more");
        assert!(e.dirty());
        e.body = TextAreaEdit::new("echo");
        e.title.insert("2");
        assert!(e.dirty());
    }

    #[test]
    fn save_is_blocked_until_the_snippet_is_valid() {
        let mut e = blank();
        assert!(e.save_blocker(false).unwrap().contains("title"));
        e.title.insert("T");
        assert!(e.save_blocker(false).unwrap().contains("body"));
        e.body.insert("echo {{ nope }}");
        // Literal mode: template text is just text.
        assert_eq!(e.save_blocker(false), None);
        // Template mode: the parse error blocks Save, with its position.
        let b = e.save_blocker(true).unwrap();
        assert!(b.contains("1:9") && b.contains("nope"), "{b}");
        e.body = TextAreaEdit::new("echo {{ uuid }}");
        assert_eq!(e.save_blocker(true), None);
        e.key.insert("cmd+nonsense+x");
        assert!(e.save_blocker(true).unwrap().starts_with("Key:"));
        e.key = TextEdit::new("cmd+alt+1");
        assert_eq!(e.save_blocker(true), None);
    }

    #[test]
    fn new_snippets_derive_ids_that_avoid_the_set() {
        let mut e = blank();
        e.title.insert("Tail Logs");
        e.body.insert("x");
        let set = SnippetSet {
            snippets: vec![Snippet {
                id: "tail-logs".into(),
                ..Default::default()
            }],
        };
        assert_eq!(e.to_snippet(&set).id, "tail-logs-2");
        assert_eq!(
            editor("x").to_snippet(&set).id,
            "a",
            "an edited snippet keeps its id"
        );
    }

    #[test]
    fn to_snippet_trims_and_splits_tags() {
        let mut e = editor("x");
        e.tags.insert(" a , ,b,c ");
        e.folder.insert("  k8s ");
        let s = e.to_snippet(&SnippetSet::default());
        assert_eq!(s.tags, ["a", "b", "c"]);
        assert_eq!(s.folder, "k8s");
        assert!(s.explicit_id);
    }

    #[test]
    fn fields_cycle() {
        let mut e = blank();
        assert_eq!(e.field, EditorField::Title);
        e.focus_next(false);
        assert_eq!(e.field, EditorField::Folder);
        e.focus_next(true);
        e.focus_next(true);
        assert_eq!(e.field, EditorField::Key, "shift-tab wraps");
        assert!(e.line_edit().is_some());
        e.field = EditorField::Body;
        assert!(e.line_edit().is_none());
    }

    #[test]
    fn diagnostics_only_with_templates_on() {
        let e = editor("{{ nope");
        assert!(e.diagnostics(false).is_empty());
        assert_eq!(e.diagnostics(true).len(), 1);
    }

    #[test]
    fn cursor_warning_for_multiline_bodies() {
        assert!(editor("a\n{{ cursor }}").cursor_warning(true).is_some());
        assert!(editor("a {{ cursor }}").cursor_warning(true).is_none());
        assert!(editor("a\n{{ cursor }}").cursor_warning(false).is_none());
    }
}
