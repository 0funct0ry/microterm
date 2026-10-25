//! `triggers.toml` write-back: surgical `toml_edit` patching through the shared
//! `mt_config::aot` helpers (§10.11.2, D11/D12). Comments and untouched rules stay
//! byte-identical.

use std::path::Path;

use mt_config::aot;
use mt_config::edit::EditError;
use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table, Value};

use crate::model::{Action, MatchIn, Rule, When};
use crate::rules::{load_str, LoadOpts};

const TABLE: &str = "trigger";
const WHAT: &str = "rule";

pub type Base = aot::Base;

fn loaded(src: &str) -> Result<Vec<Rule>, EditError> {
    load_str(
        src,
        LoadOpts {
            max_rules: usize::MAX,
        },
    )
    .map(|l| l.rules)
    .map_err(|d| EditError::Unparsable(d[0].render("triggers.toml")))
}

fn ids_of(rules: &[Rule]) -> Vec<Option<String>> {
    rules.iter().map(|r| Some(r.id.clone())).collect()
}

fn parse_mut(src: &str) -> Result<DocumentMut, EditError> {
    src.parse::<DocumentMut>()
        .map_err(|e| EditError::Unparsable(e.to_string()))
}

/// Source text of the rule `id` (for per-entry staleness).
pub fn entry_text(src: &str, id: &str) -> Option<String> {
    let ids = ids_of(&loaded(src).ok()?);
    aot::entry_text(src, TABLE, &ids, id)
}

pub fn base_of(src: &str, id: Option<&str>) -> Base {
    aot::Base::new(src, id.and_then(|i| entry_text(src, i)))
}

/// A position-less table holding the serialised form of `v` (an action or a `when`), so the
/// encoder prints it right after its parent.
fn plain_table<T: serde::Serialize>(v: &T) -> Table {
    let text = toml::to_string(v).expect("rule parts serialise");
    let doc: DocumentMut = text.parse().expect("serialised toml parses");
    let mut t = Table::new();
    for (k, item) in doc.as_table().iter() {
        t.insert(k, item.clone());
    }
    t
}

fn sync_scalars(table: &mut Table, rule: &Rule, cur: Option<&Rule>) {
    let explicit_id = table.get("id").and_then(Item::as_str) == Some(rule.id.as_str());
    if !explicit_id {
        aot::set_value(table, "id", Value::from(rule.id.clone()));
    }
    let set_or_drop = |table: &mut Table, key: &str, changed: bool, default: bool, v: Value| {
        if !changed && (table.contains_key(key) || default) {
            return;
        }
        if default {
            table.remove(key);
        } else {
            aot::set_value(table, key, v);
        }
    };
    set_or_drop(
        table,
        "name",
        cur.is_none_or(|c| c.name != rule.name),
        false,
        Value::from(rule.name.clone()),
    );
    set_or_drop(
        table,
        "enabled",
        cur.is_none_or(|c| c.enabled != rule.enabled),
        rule.enabled,
        Value::from(rule.enabled),
    );
    set_or_drop(
        table,
        "panes",
        cur.is_none_or(|c| c.panes != rule.panes),
        rule.panes == "*",
        Value::from(rule.panes.clone()),
    );
    set_or_drop(
        table,
        "scope",
        cur.is_none_or(|c| c.scope != rule.scope),
        rule.scope == MatchIn::Output,
        Value::from("any"),
    );
    set_or_drop(
        table,
        "cooldown_ms",
        cur.is_none_or(|c| c.cooldown_ms != rule.cooldown_ms),
        rule.cooldown_ms == 1000,
        Value::from(i64::from(rule.cooldown_ms)),
    );
}

fn apply(table: &mut Table, rule: &Rule, cur: Option<&Rule>) {
    sync_scalars(table, rule, cur);
    let when_changed = cur.is_none_or(|c| c.when != rule.when) || !table.contains_key("when");
    if when_changed {
        table.insert("when", Item::Table(plain_table::<When>(&rule.when)));
    }
    let actions_changed =
        cur.is_none_or(|c| c.actions != rule.actions) || !table.contains_key("actions");
    if actions_changed {
        table.remove("actions");
        let mut aot = ArrayOfTables::new();
        for a in &rule.actions {
            aot.push(plain_table::<Action>(a));
        }
        table.insert("actions", Item::ArrayOfTables(aot));
    }
}

/// Inserts or updates the rule whose id is `rule.id` (which must be set).
pub fn upsert(src: &str, rule: &Rule) -> Result<String, EditError> {
    let rules = loaded(src)?;
    let ids = ids_of(&rules);
    let pos = ids
        .iter()
        .position(|x| x.as_deref() == Some(rule.id.as_str()));
    let mut doc = parse_mut(src)?;
    if rules.is_empty() && doc.get("version").is_none() {
        doc.insert("version", value(1));
    }
    let table = aot::entry_mut(&mut doc, TABLE, pos)?;
    apply(table, rule, pos.map(|i| &rules[i]));
    Ok(doc.to_string())
}

/// Removes the rule with `id`.
pub fn remove(src: &str, id: &str) -> Result<String, EditError> {
    let ids = ids_of(&loaded(src)?);
    let mut doc = parse_mut(src)?;
    aot::remove(&mut doc, TABLE, &ids, id, WHAT)?;
    Ok(doc.to_string())
}

/// Moves `id` so it sits immediately before `before` (`None` = to the end).
pub fn reorder(src: &str, id: &str, before: Option<&str>) -> Result<String, EditError> {
    let ids = ids_of(&loaded(src)?);
    let mut doc = parse_mut(src)?;
    aot::reorder(&mut doc, TABLE, &ids, id, before, WHAT)?;
    Ok(doc.to_string())
}

fn save_with(
    path: &Path,
    base: &Base,
    id: &str,
    edit: impl FnOnce(&str) -> Result<String, EditError>,
) -> Result<String, EditError> {
    aot::save_with(path, base, |src| entry_text(src, id), edit)
}

pub fn save_rule(path: &Path, base: &Base, rule: &Rule) -> Result<String, EditError> {
    save_with(path, base, &rule.id, |src| upsert(src, rule))
}

pub fn remove_rule(path: &Path, base: &Base, id: &str) -> Result<String, EditError> {
    save_with(path, base, id, |src| remove(src, id))
}

#[cfg(test)]
mod tests;
