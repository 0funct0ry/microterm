//! Splits an `sh` input line into statements (SPEC §12.7 grammar).
//!
//! Words follow shell quoting (`'…'` literal, `"…"` with `\"`, `\\`, `\$` escapes and variable
//! expansion, `\` outside quotes). Unquoted `;`, `&&` and `||` separate statements; the first `|`
//! hands the rest of the line to a host command as a pipe; `> FILE` / `>> FILE` redirect. A line
//! starting with `!` is a host command. Variables (`$NAME`, `${NAME}`, `$?`) expand outside single
//! quotes; the result is never re-split.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Conn {
    /// First statement, or after `;`.
    #[default]
    Seq,
    /// Runs only when the previous statement succeeded.
    And,
    /// Runs only when the previous statement failed.
    Or,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    pub path: String,
    pub append: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stmt {
    pub conn: Conn,
    pub words: Vec<String>,
    /// `!cmd`: a host command line (then `words` is empty).
    pub host: Option<String>,
    /// `| cmd`: the host command that receives the rendered output.
    pub pipe: Option<String>,
    pub redirect: Option<Redirect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexError {
    /// The line continues: an open quote, a trailing `\`, or a trailing `&&` / `||`.
    Incomplete(&'static str),
    Bad(String),
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LexError::Incomplete(what) => write!(f, "unterminated {what}"),
            LexError::Bad(m) => f.write_str(m),
        }
    }
}

struct Lexer<'a> {
    chars: Vec<char>,
    i: usize,
    vars: &'a dyn Fn(&str) -> Option<String>,
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

impl Lexer<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn rest(&self) -> String {
        self.chars[self.i..].iter().collect()
    }

    /// Reads a variable reference after the `$`.
    fn variable(&mut self, out: &mut String) {
        match self.peek() {
            Some('?') => {
                self.i += 1;
                out.push_str(&(self.vars)("?").unwrap_or_default());
            }
            Some('{') => {
                let close = self.chars[self.i..].iter().position(|&c| c == '}');
                match close {
                    Some(n) => {
                        let name: String = self.chars[self.i + 1..self.i + n].iter().collect();
                        self.i += n + 1;
                        out.push_str(&(self.vars)(&name).unwrap_or_default());
                    }
                    None => out.push('$'),
                }
            }
            Some(c) if is_name_start(c) => {
                let mut name = String::new();
                while let Some(c) = self
                    .peek()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                {
                    name.push(c);
                    self.i += 1;
                }
                out.push_str(&(self.vars)(&name).unwrap_or_default());
            }
            _ => out.push('$'),
        }
    }

    fn single_quoted(&mut self, out: &mut String) -> Result<(), LexError> {
        loop {
            match self.peek() {
                None => return Err(LexError::Incomplete("quote")),
                Some('\'') => {
                    self.i += 1;
                    return Ok(());
                }
                Some(c) => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }

    fn double_quoted(&mut self, out: &mut String) -> Result<(), LexError> {
        loop {
            match self.peek() {
                None => return Err(LexError::Incomplete("quote")),
                Some('"') => {
                    self.i += 1;
                    return Ok(());
                }
                Some('\\') => {
                    self.i += 1;
                    match self.peek() {
                        Some(c @ ('"' | '\\' | '$')) => {
                            out.push(c);
                            self.i += 1;
                        }
                        Some(_) => out.push('\\'),
                        None => return Err(LexError::Incomplete("quote")),
                    }
                }
                Some('$') => {
                    self.i += 1;
                    self.variable(out);
                }
                Some(c) => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }

    /// Reads one word starting at a non-space, non-operator character.
    fn word(&mut self) -> Result<String, LexError> {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            match c {
                c if c.is_whitespace() => break,
                ';' | '|' | '>' => break,
                '&' if self.chars.get(self.i + 1) == Some(&'&') => break,
                '\'' => {
                    self.i += 1;
                    self.single_quoted(&mut out)?;
                }
                '"' => {
                    self.i += 1;
                    self.double_quoted(&mut out)?;
                }
                '\\' => {
                    self.i += 1;
                    match self.peek() {
                        Some(n) => {
                            out.push(n);
                            self.i += 1;
                        }
                        None => return Err(LexError::Incomplete("continuation")),
                    }
                }
                '$' => {
                    self.i += 1;
                    self.variable(&mut out);
                }
                // `~` and `~/…` at the start of a word are the home directory.
                '~' if out.is_empty()
                    && self
                        .chars
                        .get(self.i + 1)
                        .is_none_or(|n| *n == '/' || n.is_whitespace()) =>
                {
                    out.push_str(&(self.vars)("HOME").unwrap_or_else(|| "~".into()));
                    self.i += 1;
                }
                c => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
        Ok(out)
    }
}

fn finish(stmts: &mut Vec<Stmt>, cur: &mut Stmt, next: Conn) {
    let done = std::mem::replace(
        cur,
        Stmt {
            conn: next,
            ..Stmt::default()
        },
    );
    if !done.words.is_empty() || done.host.is_some() {
        stmts.push(done);
    }
}

/// Parses `line`; `vars` resolves `$NAME` (`?` for the last exit code).
pub fn parse(line: &str, vars: &dyn Fn(&str) -> Option<String>) -> Result<Vec<Stmt>, LexError> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(Vec::new());
    }
    if let Some(cmd) = trimmed.strip_prefix('!') {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            return Err(LexError::Bad("`!` needs a host command".into()));
        }
        return Ok(vec![Stmt {
            host: Some(cmd.to_string()),
            ..Stmt::default()
        }]);
    }
    let mut lx = Lexer {
        chars: line.chars().collect(),
        i: 0,
        vars,
    };
    let mut stmts = Vec::new();
    let mut cur = Stmt::default();
    // After `&&` / `||` a statement must follow.
    let mut dangling = false;
    // After `>` / `>>` the next word is the file.
    let mut redirect: Option<bool> = None;
    while let Some(c) = lx.peek() {
        if c.is_whitespace() {
            lx.i += 1;
            continue;
        }
        match c {
            '#' if redirect.is_none() => break,
            ';' => {
                lx.i += 1;
                if redirect.is_some() {
                    return Err(LexError::Bad("missing file after `>`".into()));
                }
                finish(&mut stmts, &mut cur, Conn::Seq);
                dangling = false;
            }
            '&' | '|' if lx.chars.get(lx.i + 1) == Some(&c) => {
                lx.i += 2;
                if redirect.is_some() {
                    return Err(LexError::Bad("missing file after `>`".into()));
                }
                if cur.words.is_empty() {
                    return Err(LexError::Bad(format!("unexpected `{c}{c}`")));
                }
                finish(
                    &mut stmts,
                    &mut cur,
                    if c == '&' { Conn::And } else { Conn::Or },
                );
                dangling = true;
            }
            '|' => {
                lx.i += 1;
                let host = lx.rest().trim().to_string();
                if host.is_empty() || cur.words.is_empty() {
                    return Err(LexError::Bad("`|` needs a command on each side".into()));
                }
                cur.pipe = Some(host);
                lx.i = lx.chars.len();
                dangling = false;
            }
            '>' => {
                lx.i += 1;
                let append = lx.peek() == Some('>');
                if append {
                    lx.i += 1;
                }
                if cur.words.is_empty() {
                    return Err(LexError::Bad("`>` needs a command before it".into()));
                }
                redirect = Some(append);
                dangling = false;
            }
            _ => {
                let w = lx.word()?;
                dangling = false;
                match redirect.take() {
                    Some(append) => cur.redirect = Some(Redirect { path: w, append }),
                    None => cur.words.push(w),
                }
            }
        }
    }
    if redirect.is_some() {
        return Err(LexError::Bad("missing file after `>`".into()));
    }
    if dangling {
        return Err(LexError::Incomplete("operator"));
    }
    finish(&mut stmts, &mut cur, Conn::Seq);
    Ok(stmts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_vars(_: &str) -> Option<String> {
        None
    }

    fn words(line: &str) -> Vec<String> {
        parse(line, &no_vars).unwrap().remove(0).words
    }

    #[test]
    fn splits_words_with_shell_quoting() {
        assert_eq!(words("pane get 7"), ["pane", "get", "7"]);
        assert_eq!(
            words(r#"tab new --title "a b" 'c d'"#),
            ["tab", "new", "--title", "a b", "c d"]
        );
        assert_eq!(words(r"a\ b c"), ["a b", "c"]);
        assert_eq!(words(r#"say "he said \"hi\"""#), ["say", r#"he said "hi""#]);
        assert_eq!(
            words("pane list 'title:*cargo*,!exited'"),
            ["pane", "list", "title:*cargo*,!exited"]
        );
    }

    #[test]
    fn variables_expand_outside_single_quotes_only() {
        let vars = |n: &str| match n {
            "pane" => Some("12".to_string()),
            "?" => Some("3".to_string()),
            _ => None,
        };
        let s = parse(r#"pane get $pane ${pane}x "$?" '$pane' $missing."#, &vars).unwrap();
        assert_eq!(s[0].words, ["pane", "get", "12", "12x", "3", "$pane", "."]);
    }

    #[test]
    fn a_leading_tilde_is_the_home_directory() {
        let vars = |n: &str| (n == "HOME").then(|| "/home/me".to_string());
        let s = parse("tab new --cwd ~/src --title a~b ~x", &vars).unwrap();
        assert_eq!(
            s[0].words,
            [
                "tab",
                "new",
                "--cwd",
                "/home/me/src",
                "--title",
                "a~b",
                "~x"
            ]
        );
    }

    #[test]
    fn separators_and_connectives() {
        let s = parse("list; pane list && tab list || window list", &no_vars).unwrap();
        let conns: Vec<_> = s.iter().map(|s| s.conn).collect();
        assert_eq!(conns, [Conn::Seq, Conn::Seq, Conn::And, Conn::Or]);
        assert_eq!(s[3].words, ["window", "list"]);
        // Operators inside quotes are text.
        assert_eq!(
            words("pane send-text 'a; b && c | d'"),
            ["pane", "send-text", "a; b && c | d"]
        );
    }

    #[test]
    fn host_pipes_redirects_and_bang() {
        let s = parse("pane get-text 7 | grep -c foo", &no_vars).unwrap();
        assert_eq!(s[0].words, ["pane", "get-text", "7"]);
        assert_eq!(s[0].pipe.as_deref(), Some("grep -c foo"));
        let s = parse("list >> out.txt", &no_vars).unwrap();
        assert_eq!(
            s[0].redirect,
            Some(Redirect {
                path: "out.txt".into(),
                append: true
            })
        );
        let s = parse("  !ls -l | wc -l", &no_vars).unwrap();
        assert_eq!(s[0].host.as_deref(), Some("ls -l | wc -l"));
        assert!(s[0].words.is_empty());
    }

    #[test]
    fn incomplete_input_asks_for_more() {
        assert_eq!(
            parse("tab new 'abc", &no_vars),
            Err(LexError::Incomplete("quote"))
        );
        assert_eq!(
            parse("tab new abc\\", &no_vars),
            Err(LexError::Incomplete("continuation"))
        );
        assert_eq!(
            parse("list &&", &no_vars),
            Err(LexError::Incomplete("operator"))
        );
    }

    #[test]
    fn malformed_lines_are_errors() {
        assert!(matches!(parse("&& list", &no_vars), Err(LexError::Bad(_))));
        assert!(matches!(parse("list >", &no_vars), Err(LexError::Bad(_))));
        assert!(matches!(parse("list |", &no_vars), Err(LexError::Bad(_))));
        assert!(matches!(parse("!", &no_vars), Err(LexError::Bad(_))));
    }

    #[test]
    fn blank_lines_and_comments_are_empty() {
        assert!(parse("", &no_vars).unwrap().is_empty());
        assert!(parse("   # nothing", &no_vars).unwrap().is_empty());
        assert_eq!(words("list # trailing comment"), ["list"]);
        assert_eq!(words("list ;; ;"), ["list"]);
    }
}
