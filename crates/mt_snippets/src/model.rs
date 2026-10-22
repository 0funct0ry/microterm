//! The snippet data model (SPEC §10.8.2). No GPUI.

pub const MAX_BODY_BYTES: usize = 64 * 1024;
pub const MAX_SNIPPETS: usize = 5_000;
pub const MAX_ID_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snippet {
    /// Final id: explicit, or derived from the title (see [`slugify`]).
    pub id: String,
    /// The file carries an explicit `id` for this entry.
    pub explicit_id: bool,
    pub title: String,
    pub body: String,
    /// Group header in the panel; empty = ungrouped.
    pub folder: String,
    pub tags: Vec<String>,
    pub description: String,
    /// Press Enter after inserting.
    pub run: bool,
    /// Optional key binding to `InsertSnippet{id}` (§9.1 syntax); empty = none.
    pub keys: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SnippetSet {
    /// File order is display order.
    pub snippets: Vec<Snippet>,
}

impl SnippetSet {
    pub fn get(&self, id: &str) -> Option<&Snippet> {
        self.snippets.iter().find(|s| s.id == id)
    }

    /// Folders in the order of their first snippet. The ungrouped folder (`""`) is included
    /// where its first snippet appears.
    pub fn folders(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for s in &self.snippets {
            if !out.contains(&s.folder.as_str()) {
                out.push(&s.folder);
            }
        }
        out
    }

    pub fn in_folder<'a>(&'a self, folder: &'a str) -> impl Iterator<Item = &'a Snippet> {
        self.snippets.iter().filter(move |s| s.folder == folder)
    }
}

/// `^[a-z0-9][a-z0-9_-]{0,63}$`.
pub fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    let ok_first = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    ok_first
        && id.len() <= MAX_ID_LEN
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Lowercase, runs of anything but `[a-z0-9_]` become one `-`, trimmed, at most 64 bytes;
/// an empty result is `snippet`.
pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() || c == '_' {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c);
        } else {
            pending_dash = true;
        }
    }
    let out: String = out
        .trim_matches(['-', '_'])
        .chars()
        .take(MAX_ID_LEN)
        .collect();
    let out = out.trim_end_matches(['-', '_']).to_string();
    if out.is_empty() {
        "snippet".into()
    } else {
        out
    }
}

/// `base`, then `base-2`, `base-3`… until `taken` does not contain it (staying within 64 bytes).
pub fn unique_id(base: &str, taken: &dyn Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    unique_id_from(base, 2, taken)
}

/// Like [`unique_id`] but starts trying suffixes at `-start` (`base` itself is not retried).
pub fn unique_id_from(base: &str, start: usize, taken: &dyn Fn(&str) -> bool) -> String {
    for n in start.max(2).. {
        let suffix = format!("-{n}");
        let keep = MAX_ID_LEN - suffix.len();
        let stem: String = base.chars().take(keep).collect();
        let cand = format!("{}{suffix}", stem.trim_end_matches(['-', '_']));
        if !taken(&cand) {
            return cand;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_rules() {
        for ok in ["a", "k8s-logs", "0x", "a_b-c", &"a".repeat(64)] {
            assert!(valid_id(ok), "{ok}");
        }
        for bad in ["", "-a", "_a", "A", "a b", "é", &"a".repeat(65)] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("Tail pod logs"), "tail-pod-logs");
        assert_eq!(slugify("  --Hello,  World!!  "), "hello-world");
        assert_eq!(slugify("a_b C"), "a_b-c");
        assert_eq!(slugify("***"), "snippet");
        assert_eq!(slugify("Ünïcode"), "n-code");
        assert_eq!(slugify(&"x".repeat(100)).len(), 64);
        assert!(valid_id(&slugify("Git: rebase -i HEAD~3")));
    }

    #[test]
    fn collisions_get_numeric_suffixes() {
        let taken = ["a", "a-2"];
        assert_eq!(unique_id("a", &|c| taken.contains(&c)), "a-3");
        assert_eq!(unique_id("b", &|c| taken.contains(&c)), "b");
        let long = "x".repeat(64);
        let id = unique_id(&long, &|c| c == long);
        assert_eq!(id.len(), 64);
        assert!(id.ends_with("-2") && valid_id(&id));
    }

    #[test]
    fn folders_follow_first_appearance() {
        let mk = |id: &str, f: &str| Snippet {
            id: id.into(),
            folder: f.into(),
            ..Default::default()
        };
        let set = SnippetSet {
            snippets: vec![mk("a", "k8s"), mk("b", ""), mk("c", "k8s"), mk("d", "git")],
        };
        assert_eq!(set.folders(), ["k8s", "", "git"]);
        assert_eq!(
            set.in_folder("k8s")
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "c"]
        );
    }
}
