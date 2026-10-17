//! `~/.ssh/config` subset (SPEC §11.2): `Host` patterns, `HostName`, `User`, `Port`,
//! `IdentityFile`, `IdentitiesOnly`, `ProxyJump`, `ProxyCommand`, `ServerAliveInterval`.
//!
//! As in OpenSSH, the first value obtained for an option wins (`IdentityFile` accumulates).
//! `Match` blocks and `Include` are not supported: `Match` blocks are skipped.

use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostSettings {
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_files: Vec<String>,
    pub identities_only: Option<bool>,
    /// `None` = not set; `Some(vec![])` = `ProxyJump none`.
    pub proxy_jump: Option<Vec<String>>,
    pub proxy_command: Option<String>,
    pub server_alive_interval: Option<u32>,
}

#[derive(Debug, Clone, Default)]
struct Block {
    /// Patterns of the `Host` line; empty for the global section before any `Host`.
    patterns: Vec<String>,
    /// `Match` blocks never apply.
    skip: bool,
    options: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct SshConfig {
    blocks: Vec<Block>,
}

/// Splits `Key value`, `Key=value` and `Key = value`, honouring double quotes in the value.
fn split_line(line: &str) -> Option<(String, Vec<String>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let key = line[..end].to_ascii_lowercase();
    let rest = line[end..].trim_start_matches(|c: char| c.is_whitespace() || c == '=');
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for ch in rest.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            '#' if !quoted => break,
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    args.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        args.push(cur);
    }
    Some((key, args))
}

/// OpenSSH wildcard match: `*` and `?`.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some((b'*', rest)) => (0..=t.len()).any(|i| go(rest, &t[i..])),
            Some((b'?', rest)) => !t.is_empty() && go(rest, &t[1..]),
            Some((c, rest)) => {
                t.first().is_some_and(|x| x.eq_ignore_ascii_case(c)) && go(rest, &t[1..])
            }
        }
    }
    go(pattern.as_bytes(), text.as_bytes())
}

/// Does a `Host` pattern list match `name`? A matching `!pattern` rejects the whole line.
fn host_matches(patterns: &[String], name: &str) -> bool {
    let mut positive = false;
    for p in patterns {
        if let Some(neg) = p.strip_prefix('!') {
            if glob_match(neg, name) {
                return false;
            }
        } else if glob_match(p, name) {
            positive = true;
        }
    }
    positive
}

impl SshConfig {
    pub fn parse(text: &str) -> SshConfig {
        let mut blocks = vec![Block::default()];
        for line in text.lines() {
            let Some((key, args)) = split_line(line) else {
                continue;
            };
            match key.as_str() {
                "host" => blocks.push(Block {
                    patterns: args,
                    ..Block::default()
                }),
                "match" => blocks.push(Block {
                    skip: true,
                    ..Block::default()
                }),
                _ => {
                    // ProxyCommand takes the rest of the line; everything else one value.
                    let v = if key == "proxycommand" {
                        Some(args.join(" ")).filter(|v| !v.is_empty())
                    } else {
                        args.into_iter().next()
                    };
                    if let Some(v) = v {
                        blocks.last_mut().unwrap().options.push((key, v));
                    }
                }
            }
        }
        SshConfig { blocks }
    }

    /// Reads `~/.ssh/config`; a missing file is an empty config.
    pub fn load_default() -> SshConfig {
        let Some(home) = std::env::var_os("HOME") else {
            return SshConfig::default();
        };
        std::fs::read_to_string(PathBuf::from(home).join(".ssh/config"))
            .map(|t| SshConfig::parse(&t))
            .unwrap_or_default()
    }

    /// Settings for `alias` (the name the user typed), first value wins.
    pub fn resolve(&self, alias: &str) -> HostSettings {
        let mut s = HostSettings::default();
        for b in &self.blocks {
            let applies = !b.skip && (b.patterns.is_empty() || host_matches(&b.patterns, alias));
            if !applies {
                continue;
            }
            for (k, v) in &b.options {
                match k.as_str() {
                    "hostname" => s.hostname = s.hostname.take().or_else(|| Some(v.clone())),
                    "user" => s.user = s.user.take().or_else(|| Some(v.clone())),
                    "port" => {
                        if s.port.is_none() {
                            s.port = v.parse().ok();
                        }
                    }
                    "identityfile" => s.identity_files.push(v.clone()),
                    "identitiesonly" => {
                        if s.identities_only.is_none() {
                            s.identities_only = Some(v.eq_ignore_ascii_case("yes"));
                        }
                    }
                    "proxyjump" => {
                        if s.proxy_jump.is_none() {
                            s.proxy_jump = Some(if v.eq_ignore_ascii_case("none") {
                                Vec::new()
                            } else {
                                v.split(',').map(|h| h.trim().to_string()).collect()
                            });
                        }
                    }
                    "proxycommand" => {
                        if s.proxy_command.is_none() {
                            s.proxy_command = Some(v.clone());
                        }
                    }
                    "serveraliveinterval" => {
                        if s.server_alive_interval.is_none() {
                            s.server_alive_interval = v.parse().ok();
                        }
                    }
                    _ => {}
                }
            }
        }
        s
    }
}

/// `[user@]host[:port]`, also `[v6]:port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hop {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

impl Hop {
    pub fn parse(s: &str) -> Hop {
        let (user, rest) = match s.split_once('@') {
            Some((u, r)) => (Some(u.to_string()), r),
            None => (None, s),
        };
        if let Some(r) = rest.strip_prefix('[') {
            if let Some((host, tail)) = r.split_once(']') {
                return Hop {
                    user,
                    host: host.into(),
                    port: tail.strip_prefix(':').and_then(|p| p.parse().ok()),
                };
            }
        }
        match rest.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') && p.parse::<u16>().is_ok() => Hop {
                user,
                host: h.into(),
                port: p.parse().ok(),
            },
            _ => Hop {
                user,
                host: rest.into(),
                port: None,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proxy {
    None,
    /// Intermediate hops, outermost first, each tunnelled through the previous one.
    Jump(Vec<Target>),
    Command(String),
}

/// Everything needed to open one SSH connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Address to connect to (after `HostName`).
    pub host: String,
    pub port: u16,
    pub user: String,
    /// Explicit identity files, `~` expanded, in preference order.
    pub identity_files: Vec<PathBuf>,
    pub identities_only: bool,
    pub proxy: Proxy,
    pub keepalive_secs: u32,
}

/// The fields of `[[ssh_domains]]` that influence a connection (SPEC §11.1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DomainSpec {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
}

pub fn expand_tilde(p: &str) -> PathBuf {
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(p),
    }
}

fn current_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "root".into())
}

/// `%h` host, `%p` port, `%r` user, `%%`.
pub fn expand_proxy_command(cmd: &str, host: &str, port: u16, user: &str) -> String {
    let mut out = String::new();
    let mut it = cmd.chars();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('h') => out.push_str(host),
            Some('p') => out.push_str(&port.to_string()),
            Some('r') => out.push_str(user),
            Some('%') => out.push('%'),
            Some(o) => {
                out.push('%');
                out.push(o);
            }
            None => out.push('%'),
        }
    }
    out
}

impl SshConfig {
    /// Resolves a domain (or ad-hoc `user@host:port`) into a connection target. Explicit
    /// domain fields beat `~/.ssh/config`, which beats defaults.
    pub fn target(&self, spec: &DomainSpec) -> Target {
        self.target_inner(spec, 0)
    }

    fn target_inner(&self, spec: &DomainSpec, depth: usize) -> Target {
        let s = self.resolve(&spec.host);
        let host = s.hostname.clone().unwrap_or_else(|| spec.host.clone());
        let port = spec.port.or(s.port).unwrap_or(22);
        let user = spec
            .user
            .clone()
            .or(s.user.clone())
            .unwrap_or_else(current_user);
        let mut identity_files: Vec<PathBuf> =
            spec.identity_file.iter().map(|f| expand_tilde(f)).collect();
        identity_files.extend(s.identity_files.iter().map(|f| expand_tilde(f)));
        let proxy = if let Some(cmd) = s.proxy_command.filter(|_| s.proxy_jump.is_none()) {
            Proxy::Command(expand_proxy_command(&cmd, &host, port, &user))
        } else if let Some(hops) = s.proxy_jump.filter(|h| !h.is_empty() && depth < 8) {
            Proxy::Jump(
                hops.iter()
                    .map(|h| {
                        let hop = Hop::parse(h);
                        self.target_inner(
                            &DomainSpec {
                                host: hop.host,
                                user: hop.user,
                                port: hop.port,
                                identity_file: None,
                            },
                            depth + 1,
                        )
                    })
                    .collect(),
            )
        } else {
            Proxy::None
        };
        Target {
            host,
            port,
            user,
            identity_files,
            identities_only: s.identities_only.unwrap_or(false),
            proxy,
            keepalive_secs: s.server_alive_interval.filter(|n| *n > 0).unwrap_or(10),
        }
    }
}

/// `[user@]host[:port]` as typed in `microterm ssh` / "Connect to host…".
pub fn parse_destination(s: &str) -> DomainSpec {
    let hop = Hop::parse(s);
    DomainSpec {
        host: hop.host,
        user: hop.user,
        port: hop.port,
        identity_file: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CFG: &str = r#"
# comment
Host prod
    HostName prod.example.com
    User bruce
    Port 2222
    IdentityFile ~/.ssh/prod_ed25519
    IdentityFile ~/.ssh/other
    IdentitiesOnly yes

Host web-* !web-dev
    User deploy
    ProxyJump jump@bastion:2200,second

Host jump
  HostName=jump.example.com

Host old
    ProxyCommand ssh -W %h:%p gate
    ServerAliveInterval 30

Match host never
    User nope

Host *
    User fallback
    Port 22
    IdentityFile ~/.ssh/id_default
    ServerAliveInterval 60
"#;

    #[test]
    fn first_value_wins_and_identity_files_accumulate() {
        let c = SshConfig::parse(CFG);
        let s = c.resolve("prod");
        assert_eq!(s.hostname.as_deref(), Some("prod.example.com"));
        assert_eq!(s.user.as_deref(), Some("bruce"));
        assert_eq!(s.port, Some(2222));
        assert_eq!(
            s.identity_files,
            vec!["~/.ssh/prod_ed25519", "~/.ssh/other", "~/.ssh/id_default"]
        );
        assert_eq!(s.identities_only, Some(true));
        assert_eq!(s.server_alive_interval, Some(60));
    }

    #[test]
    fn patterns_negation_and_match_blocks() {
        let c = SshConfig::parse(CFG);
        assert_eq!(c.resolve("web-1").user.as_deref(), Some("deploy"));
        assert_eq!(c.resolve("web-dev").user.as_deref(), Some("fallback"));
        assert_eq!(c.resolve("other").user.as_deref(), Some("fallback"));
        assert!(glob_match("a?c*", "abcdef") && !glob_match("a?c", "ac"));
    }

    #[test]
    fn equals_syntax_and_quotes() {
        let c = SshConfig::parse("Host \"a b\" c\n  User = \"x y\"\n");
        assert_eq!(c.resolve("c").user.as_deref(), Some("x y"));
        assert_eq!(c.resolve("a b").user.as_deref(), Some("x y"));
    }

    #[test]
    fn hop_parsing() {
        assert_eq!(
            Hop::parse("u@h:2200"),
            Hop {
                user: Some("u".into()),
                host: "h".into(),
                port: Some(2200)
            }
        );
        assert_eq!(Hop::parse("[::1]:22").host, "::1");
        assert_eq!(
            Hop::parse("::1"),
            Hop {
                user: None,
                host: "::1".into(),
                port: None
            }
        );
        assert_eq!(
            parse_destination("bruce@prod").user.as_deref(),
            Some("bruce")
        );
    }

    #[test]
    fn target_merges_domain_over_config_over_defaults() {
        let c = SshConfig::parse(CFG);
        let t = c.target(&DomainSpec {
            host: "prod".into(),
            port: Some(2022),
            ..Default::default()
        });
        assert_eq!(
            (t.host.as_str(), t.port, t.user.as_str()),
            ("prod.example.com", 2022, "bruce")
        );
        assert!(t.identities_only);
        assert_eq!(t.keepalive_secs, 60);
        let t = c.target(&DomainSpec {
            host: "prod".into(),
            user: Some("me".into()),
            ..Default::default()
        });
        assert_eq!(t.user, "me");
    }

    #[test]
    fn proxy_jump_chain_resolves_each_hop_through_config() {
        let c = SshConfig::parse(CFG);
        let t = c.target(&DomainSpec {
            host: "web-3".into(),
            ..Default::default()
        });
        let Proxy::Jump(hops) = &t.proxy else {
            panic!("{:?}", t.proxy)
        };
        assert_eq!(hops.len(), 2);
        assert_eq!(
            (hops[0].host.as_str(), hops[0].port, hops[0].user.as_str()),
            ("bastion", 2200, "jump")
        );
        assert_eq!(hops[1].host, "second");
        // ProxyJump none disables it.
        let c = SshConfig::parse("Host x\n ProxyJump none\nHost *\n ProxyJump y\n");
        assert_eq!(
            c.target(&DomainSpec {
                host: "x".into(),
                ..Default::default()
            })
            .proxy,
            Proxy::None
        );
    }

    #[test]
    fn proxy_command_expands_tokens() {
        let c = SshConfig::parse(CFG);
        let t = c.target(&DomainSpec {
            host: "old".into(),
            port: Some(2),
            user: Some("u".into()),
            ..Default::default()
        });
        assert_eq!(t.proxy, Proxy::Command("ssh -W old:2 gate".into()));
        assert_eq!(t.keepalive_secs, 30);
        assert_eq!(expand_proxy_command("a %% %r@%h", "h", 1, "u"), "a % u@h");
    }
}
