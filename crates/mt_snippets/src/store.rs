//! `snippets.toml`: load with path-addressed diagnostics, and surgical write-back (SPEC §10.8.2).

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::Path;

use mt_config::aot;
use mt_config::edit::EditError;
use mt_config::Severity;
use toml_edit::{Array, DocumentMut, ImDocument, Item, Table, Value};

use crate::model::{
    slugify, unique_id, unique_id_from, valid_id, Snippet, SnippetSet, MAX_BODY_BYTES, MAX_SNIPPETS,
};

const ENTRY_KEYS: [&str; 8] = [
    "id",
    "title",
    "body",
    "folder",
    "tags",
    "description",
    "run",
    "keys",
];

/// A problem in `snippets.toml`. `path` addresses the entry (`snippets[3].body`); `line` and
/// `col` are 1-based, `line == 0` means no position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub path: String,
    pub message: String,
    pub line: usize,
    pub col: usize,
}

impl Diagnostic {
    /// `snippets.toml:12:5 — snippets[3].body: must be a string`.
    pub fn render(&self, file: &str) -> String {
        let at = if self.line == 0 {
            file.to_string()
        } else {
            format!("{file}:{}:{}", self.line, self.col)
        };
        if self.path.is_empty() {
            format!("{at} — {}", self.message)
        } else {
            format!("{at} — {}: {}", self.path, self.message)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub set: SnippetSet,
    /// Warnings, plus errors that skipped one entry (duplicate or invalid `id`).
    pub diagnostics: Vec<Diagnostic>,
}

fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(src.len());
    let before = &src[..offset];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

struct Sink<'a> {
    src: &'a str,
    out: Vec<Diagnostic>,
}

impl Sink<'_> {
    fn push(
        &mut self,
        severity: Severity,
        path: String,
        span: Option<Range<usize>>,
        message: String,
    ) {
        let (line, col) = span.map_or((0, 0), |s| line_col(self.src, s.start));
        self.out.push(Diagnostic {
            severity,
            path,
            message,
            line,
            col,
        });
    }
}

/// One parsed entry before ids are assigned.
struct Raw {
    index: usize,
    explicit_id: Option<String>,
    snippet: Snippet,
    span: Option<Range<usize>>,
}

fn suggest<'a>(unknown: &str, known: &[&'a str]) -> Option<&'a str> {
    known
        .iter()
        .map(|k| (strsim::damerau_levenshtein(unknown, k), *k))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, k)| k)
}

fn parse_raw(src: &str, doc: &ImDocument<&str>, sink: &mut Sink) -> Option<Vec<Raw>> {
    let mut fatal = false;
    for (k, _) in doc.as_table().iter() {
        if k != "snippets" {
            let span = doc
                .as_table()
                .get_key_value(k)
                .and_then(|(key, _)| key.span());
            let hint = suggest(k, &["snippets"])
                .map_or(String::new(), |s| format!(" (did you mean \"{s}\"?)"));
            sink.push(
                Severity::Warning,
                String::new(),
                span,
                format!("unknown key \"{k}\"{hint}"),
            );
        }
    }
    let Some(item) = doc.get("snippets") else {
        return Some(Vec::new());
    };
    let Some(aot) = item.as_array_of_tables() else {
        sink.push(
            Severity::Error,
            "snippets".into(),
            item.span(),
            "must be an array of tables ([[snippets]])".into(),
        );
        return None;
    };
    if aot.len() > MAX_SNIPPETS {
        sink.push(
            Severity::Error,
            "snippets".into(),
            item.span(),
            format!(
                "at most {MAX_SNIPPETS} snippets per file (found {})",
                aot.len()
            ),
        );
        return None;
    }
    let mut out = Vec::new();
    for (index, table) in aot.iter().enumerate() {
        let path = |k: &str| format!("snippets[{index}].{k}");
        for (k, _) in table.iter() {
            if !ENTRY_KEYS.contains(&k) {
                let span = table.get_key_value(k).and_then(|(key, _)| key.span());
                let hint = suggest(k, &ENTRY_KEYS)
                    .map_or(String::new(), |s| format!(" (did you mean \"{s}\"?)"));
                sink.push(
                    Severity::Warning,
                    path(k),
                    span,
                    format!("unknown key \"{k}\"{hint}"),
                );
            }
        }
        let mut s = Snippet::default();
        let mut explicit = None;
        let str_field = |key: &str, sink: &mut Sink, fatal: &mut bool| -> Option<String> {
            let item = table.get(key)?;
            match item.as_str() {
                Some(v) => Some(v.to_string()),
                None => {
                    sink.push(
                        Severity::Error,
                        path(key),
                        item.span(),
                        "must be a string".into(),
                    );
                    *fatal = true;
                    None
                }
            }
        };
        if let Some(id) = str_field("id", sink, &mut fatal) {
            explicit = Some(id);
        }
        match str_field("title", sink, &mut fatal) {
            Some(t) if t.trim().is_empty() || t.contains('\n') => {
                sink.push(
                    Severity::Error,
                    path("title"),
                    table.get("title").and_then(Item::span),
                    "must be a non-empty single line".into(),
                );
                fatal = true;
            }
            Some(t) => s.title = t,
            None if table.get("title").is_none() => {
                sink.push(
                    Severity::Error,
                    path("title"),
                    table.span(),
                    "is required".into(),
                );
                fatal = true;
            }
            None => {}
        }
        match str_field("body", sink, &mut fatal) {
            Some(b) if b.is_empty() => {
                sink.push(
                    Severity::Error,
                    path("body"),
                    table.get("body").and_then(Item::span),
                    "must not be empty".into(),
                );
                fatal = true;
            }
            Some(b) if b.len() > MAX_BODY_BYTES => {
                sink.push(
                    Severity::Error,
                    path("body"),
                    table.get("body").and_then(Item::span),
                    format!("is {} bytes; the limit is {MAX_BODY_BYTES}", b.len()),
                );
                fatal = true;
            }
            Some(b) => s.body = b.replace("\r\n", "\n"),
            None if table.get("body").is_none() => {
                sink.push(
                    Severity::Error,
                    path("body"),
                    table.span(),
                    "is required".into(),
                );
                fatal = true;
            }
            None => {}
        }
        s.folder = str_field("folder", sink, &mut fatal).unwrap_or_default();
        s.description = str_field("description", sink, &mut fatal).unwrap_or_default();
        s.keys = str_field("keys", sink, &mut fatal).unwrap_or_default();
        if let Some(item) = table.get("run") {
            match item.as_bool() {
                Some(b) => s.run = b,
                None => {
                    sink.push(
                        Severity::Error,
                        path("run"),
                        item.span(),
                        "must be true or false".into(),
                    );
                    fatal = true;
                }
            }
        }
        if let Some(item) = table.get("tags") {
            let tags: Option<Vec<String>> = item
                .as_array()
                .and_then(|a| a.iter().map(|v| v.as_str().map(str::to_string)).collect());
            match tags {
                Some(t) => s.tags = t,
                None => {
                    sink.push(
                        Severity::Error,
                        path("tags"),
                        item.span(),
                        "must be an array of strings".into(),
                    );
                    fatal = true;
                }
            }
        }
        out.push(Raw {
            index,
            explicit_id: explicit,
            snippet: s,
            span: table.span(),
        });
    }
    let _ = src;
    (!fatal).then_some(out)
}

/// Assigns final ids in file order. Explicit ids are reserved first, so a derived id never
/// steals a later explicit one. Invalid or duplicate explicit ids skip that entry (`None`).
fn assign_ids(raws: &mut [Raw], sink: &mut Sink) -> Vec<Option<String>> {
    let mut taken: HashSet<String> = HashSet::new();
    let mut next_suffix: HashMap<String, usize> = HashMap::new();
    let mut result: Vec<Option<String>> = vec![None; raws.len()];
    let mut skipped = vec![false; raws.len()];
    for (i, r) in raws.iter().enumerate() {
        let Some(id) = &r.explicit_id else { continue };
        let path = format!("snippets[{}].id", r.index);
        if !valid_id(id) {
            sink.push(
                Severity::Error,
                path,
                r.span.clone(),
                format!("invalid id \"{id}\" (use a-z, 0-9, _ and -, at most 64 characters); entry skipped"),
            );
            skipped[i] = true;
        } else if taken.contains(id.as_str()) {
            sink.push(
                Severity::Error,
                path,
                r.span.clone(),
                format!("duplicate id \"{id}\"; entry skipped"),
            );
            skipped[i] = true;
        } else {
            taken.insert(id.clone());
            result[i] = Some(id.clone());
        }
    }
    for (i, r) in raws.iter().enumerate() {
        if r.explicit_id.is_none() {
            let base = slugify(&r.snippet.title);
            // Resume after the last suffix tried for this base: linear overall, same result.
            let start = next_suffix.get(&base).copied().unwrap_or(1);
            let id = if start == 1 {
                unique_id(&base, &|c| taken.contains(c))
            } else {
                unique_id_from(&base, start, &|c| taken.contains(c))
            };
            let n = id
                .rsplit_once('-')
                .and_then(|(_, n)| n.parse::<usize>().ok())
                .filter(|_| id != base)
                .unwrap_or(1);
            next_suffix.insert(base, n);
            taken.insert(id.clone());
            result[i] = Some(id);
        }
    }
    for (i, r) in raws.iter_mut().enumerate() {
        r.snippet.explicit_id = r.explicit_id.is_some();
        if let Some(id) = &result[i] {
            r.snippet.id = id.clone();
        }
    }
    result
}

/// Parses `snippets.toml` text. `Err` means the file is unusable (syntax or type errors): the
/// caller keeps the last valid set and shows the diagnostics.
pub fn load_str(src: &str) -> Result<Loaded, Vec<Diagnostic>> {
    let mut sink = Sink {
        src,
        out: Vec::new(),
    };
    let doc = match ImDocument::parse(src) {
        Ok(d) => d,
        Err(e) => {
            sink.push(
                Severity::Error,
                String::new(),
                e.span(),
                e.message().to_string(),
            );
            return Err(sink.out);
        }
    };
    let Some(mut raws) = parse_raw(src, &doc, &mut sink) else {
        return Err(sink.out);
    };
    let ids = assign_ids(&mut raws, &mut sink);
    let snippets = raws
        .into_iter()
        .zip(ids)
        .filter_map(|(r, id)| id.map(|_| r.snippet))
        .collect();
    sink.out.sort_by_key(|d| (d.line, d.col));
    Ok(Loaded {
        set: SnippetSet { snippets },
        diagnostics: sink.out,
    })
}

/// Loads `path`. A missing file is an empty set.
pub fn load(path: &Path) -> Result<Loaded, Vec<Diagnostic>> {
    match std::fs::read_to_string(path) {
        Ok(src) => load_str(&src),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Loaded {
            set: SnippetSet::default(),
            diagnostics: Vec::new(),
        }),
        Err(e) => Err(vec![Diagnostic {
            severity: Severity::Error,
            path: String::new(),
            message: format!("cannot read file: {e}"),
            line: 0,
            col: 0,
        }]),
    }
}

// ---------------------------------------------------------------------------------------------
// Write-back

/// Final id of each `[[snippets]]` entry in `src`, by index (`None` = skipped).
fn entry_ids(src: &str) -> Result<Vec<Option<String>>, EditError> {
    let mut sink = Sink {
        src,
        out: Vec::new(),
    };
    let doc = ImDocument::parse(src).map_err(|e| EditError::Unparsable(e.to_string()))?;
    let mut raws = parse_raw(src, &doc, &mut sink).ok_or_else(|| {
        EditError::Unparsable(
            sink.out
                .first()
                .map_or_else(String::new, |d| d.render("snippets.toml")),
        )
    })?;
    Ok(assign_ids(&mut raws, &mut sink))
}

/// The source text of the `[[snippets]]` entry with `id` (for per-entry staleness).
pub fn entry_text(src: &str, id: &str) -> Option<String> {
    let ids = entry_ids(src).ok()?;
    aot::entry_text(src, "snippets", &ids, id)
}

fn parse_mut(src: &str) -> Result<DocumentMut, EditError> {
    // Never write a file that does not parse (or whose entries are unusable).
    entry_ids(src)?;
    src.parse::<DocumentMut>()
        .map_err(|e| EditError::Unparsable(e.to_string()))
}

fn literal_ok(body: &str) -> bool {
    !body.contains("'''")
        && body
            .chars()
            .all(|c| c == '\n' || c == '\t' || !c.is_control())
}

/// A string value: bodies are `'''…'''` literals when possible, else a basic string.
fn string_value(text: &str, literal: bool) -> Value {
    if literal && literal_ok(text) {
        let raw = if text.contains('\n') {
            format!("'''\n{text}'''")
        } else {
            format!("'''{text}'''")
        };
        // Parsing keeps the literal representation; verify it reads back unchanged.
        if let Ok(v) = raw.parse::<Value>() {
            if v.as_str() == Some(text) {
                return v;
            }
        }
    }
    Value::from(text.to_string())
}

fn tags_value(tags: &[String]) -> Value {
    let mut a = Array::new();
    for t in tags {
        a.push(t.as_str());
    }
    Value::Array(a)
}

/// Writes `s` into `table`, touching only the keys whose value differs from `current`.
fn apply(table: &mut Table, s: &Snippet, current: Option<&Snippet>) {
    let cur = current;
    // id is always explicit once saved from the panel.
    if cur.is_none_or(|c| !c.explicit_id) || table.get("id").and_then(Item::as_str) != Some(&s.id) {
        aot::set_value(table, "id", Value::from(s.id.clone()));
    }
    let str_field = |table: &mut Table, key: &str, new: &str, old: Option<&str>, literal: bool| {
        if Some(new) == old && table.contains_key(key) {
            return;
        }
        if new.is_empty() && key != "title" && key != "body" {
            table.remove(key);
        } else {
            aot::set_value(table, key, string_value(new, literal));
        }
    };
    str_field(
        table,
        "title",
        &s.title,
        cur.map(|c| c.title.as_str()),
        false,
    );
    str_field(table, "body", &s.body, cur.map(|c| c.body.as_str()), true);
    str_field(
        table,
        "folder",
        &s.folder,
        cur.map(|c| c.folder.as_str()),
        false,
    );
    if cur.map(|c| &c.tags) != Some(&s.tags) || (!s.tags.is_empty() && !table.contains_key("tags"))
    {
        if s.tags.is_empty() {
            table.remove("tags");
        } else {
            aot::set_value(table, "tags", tags_value(&s.tags));
        }
    }
    str_field(
        table,
        "description",
        &s.description,
        cur.map(|c| c.description.as_str()),
        false,
    );
    if cur.map(|c| c.run) != Some(s.run) || (s.run && !table.contains_key("run")) {
        if s.run {
            aot::set_value(table, "run", Value::from(true));
        } else {
            table.remove("run");
        }
    }
    str_field(table, "keys", &s.keys, cur.map(|c| c.keys.as_str()), false);
}

/// Inserts or updates the entry whose id is `s.id` (matching derived ids too), writing an
/// explicit `id`. Untouched entries, comments and key order are byte-identical.
pub fn upsert(src: &str, s: &Snippet) -> Result<String, EditError> {
    let mut doc = parse_mut(src)?;
    let ids = entry_ids(src)?;
    let current = load_str(src).ok().and_then(|l| l.set.get(&s.id).cloned());
    let pos = ids.iter().position(|x| x.as_deref() == Some(s.id.as_str()));
    let was_new = pos.is_none();
    let table = aot::entry_mut(&mut doc, "snippets", pos)?;
    apply(table, s, if was_new { None } else { current.as_ref() });
    Ok(doc.to_string())
}

/// Removes the entry with `id`.
pub fn remove(src: &str, id: &str) -> Result<String, EditError> {
    let mut doc = parse_mut(src)?;
    let ids = entry_ids(src)?;
    aot::remove(&mut doc, "snippets", &ids, id, "snippet")?;
    Ok(doc.to_string())
}

/// Moves `id` so it sits immediately before `before` (`None` = to the end).
pub fn reorder(src: &str, id: &str, before: Option<&str>) -> Result<String, EditError> {
    let mut doc = parse_mut(src)?;
    let ids = entry_ids(src)?;
    aot::reorder(&mut doc, "snippets", &ids, id, before, "snippet")?;
    Ok(doc.to_string())
}

/// What an editor remembers from when it opened an entry, for per-entry staleness (§10.8.2).
pub type Base = aot::Base;

/// `Base` for the entry `id` of `src` (`None` = a new snippet).
pub fn base_of(src: &str, id: Option<&str>) -> Base {
    aot::Base::new(src, id.and_then(|i| entry_text(src, i)))
}

/// Applies `edit` to `path` atomically. If the file changed since `base`, the edit is re-applied
/// to the fresh file only when the entry `id` is byte-identical there; otherwise `Stale`.
pub fn save_with(
    path: &Path,
    base: &Base,
    id: &str,
    edit: impl FnOnce(&str) -> Result<String, EditError>,
) -> Result<String, EditError> {
    aot::save_with(path, base, |src| entry_text(src, id), edit)
}

pub fn save_snippet(path: &Path, base: &Base, s: &Snippet) -> Result<String, EditError> {
    save_with(path, base, &s.id, |src| upsert(src, s))
}

pub fn remove_snippet(path: &Path, base: &Base, id: &str) -> Result<String, EditError> {
    save_with(path, base, id, |src| remove(src, id))
}

#[cfg(test)]
mod tests;
