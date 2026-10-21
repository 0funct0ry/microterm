//! What is under a cell: the word (selection rules) and a `path[:line[:col]]` token, used for
//! context-menu placeholders (SPEC §10.9.4). Columns are char indices into the row text.

/// The word around `col`: alphanumerics plus `word_chars`. `None` on a separator.
pub fn word_at(line: &str, col: usize, word_chars: &str) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || word_chars.contains(c);
    if !chars.get(col).copied().is_some_and(is_word) {
        return None;
    }
    let start = (0..col)
        .rev()
        .take_while(|&i| is_word(chars[i]))
        .last()
        .unwrap_or(col);
    let end = (col..chars.len())
        .take_while(|&i| is_word(chars[i]))
        .last()
        .unwrap_or(col);
    Some(chars[start..=end].iter().collect())
}

/// A file path at `col`, with the line number of a trailing `:N` (`src/main.rs:12:5`). The token
/// runs between whitespace, quotes and brackets; it counts as a path when it contains `/` or
/// a `name.ext` file name.
pub fn path_at(line: &str, col: usize) -> Option<(String, Option<u32>)> {
    let chars: Vec<char> = line.chars().collect();
    let stop = |c: char| c.is_whitespace() || "\"'`()[]{}<>,;|".contains(c);
    if chars.get(col).is_none_or(|c| stop(*c)) {
        return None;
    }
    let start = (0..col)
        .rev()
        .take_while(|&i| !stop(chars[i]))
        .last()
        .unwrap_or(col);
    let end = (col..chars.len())
        .take_while(|&i| !stop(chars[i]))
        .last()
        .unwrap_or(col);
    let token: String = chars[start..=end].iter().collect();
    let token = token.trim_end_matches(['.', ':']);
    let mut parts = token.splitn(3, ':');
    let path = parts.next()?.to_string();
    let line_no = parts.next().and_then(|n| n.parse::<u32>().ok());
    let has_ext = path
        .rsplit('/')
        .next()
        .and_then(|f| f.rsplit_once('.'))
        .is_some_and(|(stem, ext)| {
            !stem.is_empty() && !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric())
        });
    if path.is_empty() || path.contains("://") || !(path.contains('/') || has_ext) {
        return None;
    }
    Some((path, line_no))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_follow_word_chars() {
        let l = "run cargo-watch now";
        assert_eq!(word_at(l, 6, "-").as_deref(), Some("cargo-watch"));
        assert_eq!(word_at(l, 6, "").as_deref(), Some("cargo"));
        assert_eq!(word_at(l, 3, "-"), None);
        assert_eq!(word_at(l, 99, "-"), None);
        assert_eq!(word_at("ünï ok", 1, "").as_deref(), Some("ünï"));
    }

    #[test]
    fn paths_with_line_numbers() {
        let l = "error at src/main.rs:12:5: oops";
        assert_eq!(path_at(l, 12), Some(("src/main.rs".into(), Some(12))));
        assert_eq!(
            path_at("see (./a/b.txt).", 7),
            Some(("./a/b.txt".into(), None))
        );
        assert_eq!(
            path_at("open Cargo.toml", 7),
            Some(("Cargo.toml".into(), None))
        );
        assert_eq!(path_at("/etc/hosts", 0), Some(("/etc/hosts".into(), None)));
        assert_eq!(path_at("plain words", 2), None);
        assert_eq!(path_at("https://x.io/a.html", 3), None);
        assert_eq!(path_at("a b", 1), None);
    }
}
