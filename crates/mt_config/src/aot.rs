//! Surgical write-back helpers for id-keyed arrays of tables (`[[snippets]]`, `[[trigger]]`).
//!
//! The caller knows how to compute the final id of each entry (`ids`, by index; `None` = a
//! skipped entry). Everything here touches only the entry it is asked to, so comments, key order
//! and untouched entries stay byte-identical (SPEC D11/D12, §10.8.2, §10.11.2).

use std::path::Path;

use toml_edit::{ArrayOfTables, DocumentMut, ImDocument, Item, Table, Value};

use crate::edit::{content_hash, EditError};

/// The source text of the entry with `id` in `[[table]]` (for per-entry staleness).
pub fn entry_text(src: &str, table: &str, ids: &[Option<String>], id: &str) -> Option<String> {
    let doc = ImDocument::parse(src).ok()?;
    let aot = doc.get(table)?.as_array_of_tables()?;
    let i = ids.iter().position(|x| x.as_deref() == Some(id))?;
    let span = aot.get(i)?.span()?;
    src.get(span).map(str::to_string)
}

/// Sets `key` to `new`, keeping the decor (spacing, trailing comment) of the old value.
pub fn set_value(table: &mut Table, key: &str, new: Value) {
    let decor = table
        .get(key)
        .and_then(Item::as_value)
        .map(|v| v.decor().clone());
    let mut new = new;
    if let Some(d) = decor {
        *new.decor_mut() = d;
    } else {
        new.decor_mut().set_prefix(" ");
    }
    table.insert(key, Item::Value(new));
}

fn aot_mut<'a>(doc: &'a mut DocumentMut, table: &str) -> Result<&'a mut ArrayOfTables, EditError> {
    doc[table]
        .as_array_of_tables_mut()
        .ok_or_else(|| EditError::Unparsable(format!("{table} is not an array of tables")))
}

/// The mutable table at index `pos` of `[[table]]`, or a freshly appended one when `pos` is
/// `None` (creating the array when the file has none).
pub fn entry_mut<'a>(
    doc: &'a mut DocumentMut,
    table: &str,
    pos: Option<usize>,
) -> Result<&'a mut Table, EditError> {
    if doc.get(table).is_none() {
        doc.insert(table, Item::ArrayOfTables(ArrayOfTables::new()));
    }
    let aot = aot_mut(doc, table)?;
    match pos {
        Some(i) => Ok(aot.get_mut(i).expect("index from entry ids")),
        None => {
            aot.push(Table::new());
            let n = aot.len() - 1;
            Ok(aot.get_mut(n).expect("just pushed"))
        }
    }
}

/// Removes the entry with `id`.
pub fn remove(
    doc: &mut DocumentMut,
    table: &str,
    ids: &[Option<String>],
    id: &str,
    what: &str,
) -> Result<(), EditError> {
    let i = ids
        .iter()
        .position(|x| x.as_deref() == Some(id))
        .ok_or_else(|| EditError::Unparsable(format!("no {what} with id \"{id}\"")))?;
    aot_mut(doc, table)?.remove(i);
    Ok(())
}

/// Moves `id` so it sits immediately before `before` (`None` = to the end).
pub fn reorder(
    doc: &mut DocumentMut,
    table: &str,
    ids: &[Option<String>],
    id: &str,
    before: Option<&str>,
    what: &str,
) -> Result<(), EditError> {
    let find = |want: &str| ids.iter().position(|x| x.as_deref() == Some(want));
    let from =
        find(id).ok_or_else(|| EditError::Unparsable(format!("no {what} with id \"{id}\"")))?;
    let aot = aot_mut(doc, table)?;
    let mut tables: Vec<Table> = aot.iter().cloned().collect();
    // toml_edit prints array-of-tables entries by their stored position, so hand the original
    // positions back out in the new order.
    let mut positions: Vec<usize> = tables.iter().filter_map(Table::position).collect();
    positions.sort_unstable();
    let moved = tables.remove(from);
    let mut to = match before.and_then(find) {
        Some(b) if b > from => b - 1,
        Some(b) => b,
        None => tables.len(),
    };
    to = to.min(tables.len());
    tables.insert(to, moved);
    if positions.len() == tables.len() {
        for (t, p) in tables.iter_mut().zip(positions) {
            t.set_position(p);
        }
    }
    aot.clear();
    for t in tables {
        aot.push(t);
    }
    Ok(())
}

/// What an editor remembers from when it opened an entry, for per-entry staleness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base {
    pub file_hash: u64,
    /// Source text of the entry when opened; `None` for a new entry.
    pub entry: Option<String>,
}

impl Base {
    pub fn new(src: &str, entry: Option<String>) -> Self {
        Self {
            file_hash: content_hash(src),
            entry,
        }
    }
}

/// Applies `edit` to `path` atomically. If the file changed since `base`, the edit is re-applied
/// to the fresh file only when the entry (`entry_of(src)`) is byte-identical there; else `Stale`.
pub fn save_with(
    path: &Path,
    base: &Base,
    entry_of: impl Fn(&str) -> Option<String>,
    edit: impl FnOnce(&str) -> Result<String, EditError>,
) -> Result<String, EditError> {
    crate::atomic::write_atomic(path, None, |src| {
        if content_hash(src) != base.file_hash && entry_of(src) != base.entry {
            return Err(EditError::Stale);
        }
        edit(src)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "# top\n[[t]]\nid = \"a\" # keep\nx = 1\n\n[[t]]\nid = \"b\"\nx = 2\n";

    fn ids() -> Vec<Option<String>> {
        vec![Some("a".into()), Some("b".into())]
    }

    #[test]
    fn entry_text_is_the_entry_span() {
        let t = entry_text(SRC, "t", &ids(), "b").unwrap();
        assert!(t.contains("id = \"b\"") && !t.contains("\"a\""));
        assert!(entry_text(SRC, "t", &ids(), "zz").is_none());
    }

    #[test]
    fn set_value_keeps_trailing_comment() {
        let mut doc: DocumentMut = SRC.parse().unwrap();
        let t = entry_mut(&mut doc, "t", Some(0)).unwrap();
        set_value(t, "id", Value::from("a"));
        assert_eq!(doc.to_string(), SRC);
    }

    #[test]
    fn remove_and_reorder_leave_other_bytes() {
        let mut doc: DocumentMut = SRC.parse().unwrap();
        remove(&mut doc, "t", &ids(), "a", "rule").unwrap();
        assert!(!doc.to_string().contains("keep") && doc.to_string().contains("id = \"b\""));
        let mut doc: DocumentMut = SRC.parse().unwrap();
        reorder(&mut doc, "t", &ids(), "b", Some("a"), "rule").unwrap();
        let out = doc.to_string();
        assert!(out.find("\"b\"").unwrap() < out.find("\"a\"").unwrap());
        assert!(out.contains("# keep"));
    }

    #[test]
    fn missing_id_is_an_error() {
        let mut doc: DocumentMut = SRC.parse().unwrap();
        assert!(remove(&mut doc, "t", &ids(), "zz", "rule").is_err());
    }
}
