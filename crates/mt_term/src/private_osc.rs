//! The private OSC range `7700–7799` (SPEC §6.4, decision D15).
//!
//! One entry point for the whole block: [`parse`] decodes `<num>;<ns>;<verb>;<fields…>` and
//! [`PrivateState::handle`] evaluates the two layers of §6.4.2 (master switch, then feature
//! gate) **before any state is touched**. A dead sequence is swallowed and counted. The only
//! bytes the range may ever write back to the PTY are the reply to `7727;caps;query`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::menu_reg::{self, b64_decode, b64_encode, MenuRegistry, RegCmd};

/// Longest OSC string (without the introducer and terminator) the range accepts.
pub const MAX_OSC: usize = 8192;
/// Default stateful sequences per pane and second.
pub const DEFAULT_RATE: u32 = 20;
/// `7727;caps;query` replies per pane and second.
pub const CAPS_REPLIES_PER_SEC: u32 = 2;
/// Status slots per pane.
pub const STATUS_SLOTS: usize = 8;
/// Status text length in characters, after sanitising.
pub const STATUS_TEXT_MAX: usize = 48;
/// Status tooltip length in bytes.
pub const STATUS_TOOLTIP_MAX: usize = 256;
/// `set`/`clear` commands per pane and second for `7725`.
pub const STATUS_RATE_PER_SEC: u32 = 5;
/// Progress bars per pane.
pub const PROGRESS_BARS: usize = 6;
/// Progress label length in characters, after sanitising.
pub const PROGRESS_LABEL_MAX: usize = 48;
/// `7720` `set` commands per pane and second.
pub const PROGRESS_RATE_PER_SEC: u32 = 10;
/// A bar without an update for this long is dropped.
pub const PROGRESS_STALE_MS: u64 = 600_000;
/// A `done` bar shows 100 % for this long.
pub const PROGRESS_DONE_MS: u64 = 2000;
/// Largest `eta_s`.
pub const ETA_MAX: u32 = 359_999;
/// `7718` text length in characters.
pub const BADGE_TEXT_MAX: usize = 24;
/// `7718` commands per pane and second.
pub const BADGE_RATE_PER_SEC: u32 = 2;
/// `7722` note length in characters.
pub const MARK_NOTE_MAX: usize = 120;
/// `7722` marks kept per pane (oldest evicted).
pub const MARKS_MAX: usize = 500;
/// `7722` commands per pane and second.
pub const MARK_RATE_PER_SEC: u32 = 20;
/// `7723` key bindings per pane.
pub const KEY_BINDINGS_MAX: usize = 16;
/// `7724` palette entries per pane.
pub const PALETTE_ENTRIES_MAX: usize = 16;
/// `7723` `add`/`clear` commands per pane and second.
pub const KEY_RATE_PER_SEC: u32 = 5;
/// `7724` commands per pane and second, shared with `7723` and `7717`.
pub const PALETTE_RATE_PER_SEC: u32 = 10;
/// `7721` regions kept per pane (oldest dropped).
pub const FOLD_REGIONS_MAX: usize = 200;
/// `7721` nesting depth.
pub const FOLD_DEPTH_MAX: usize = 4;
/// `7721` title length in characters, after sanitising.
pub const FOLD_TITLE_MAX: usize = 80;
/// `7721` `begin`/`end` commands per pane and second.
pub const FOLD_RATE_PER_SEC: u32 = 20;
/// `7726` rules per pane.
pub const LINK_RULES_MAX: usize = 8;
/// `7726` regex length in bytes.
pub const LINK_REGEX_MAX: usize = 256;
/// `7726` template length in bytes.
pub const LINK_TEMPLATE_MAX: usize = 512;
/// `7726` `add`/`clear` commands per pane and second.
pub const LINK_RATE_PER_SEC: u32 = 5;
const WINDOW_MS: u64 = 1000;

/// One member of the range, named as in the §6.4 table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feature {
    Menu,
    Caps,
    Status,
    Progress,
    Tab,
    Badge,
    Mark,
    Key,
    Palette,
    Fold,
    Link,
}

impl Feature {
    pub const ALL: [Feature; 11] = [
        Feature::Menu,
        Feature::Caps,
        Feature::Status,
        Feature::Progress,
        Feature::Tab,
        Feature::Badge,
        Feature::Mark,
        Feature::Key,
        Feature::Palette,
        Feature::Fold,
        Feature::Link,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Feature::Menu => "menu",
            Feature::Caps => "caps",
            Feature::Status => "status",
            Feature::Progress => "progress",
            Feature::Tab => "tab",
            Feature::Badge => "badge",
            Feature::Mark => "mark",
            Feature::Key => "key",
            Feature::Palette => "palette",
            Feature::Fold => "fold",
            Feature::Link => "link",
        }
    }

    pub fn number(self) -> u16 {
        match self {
            Feature::Menu => 7717,
            Feature::Caps => 7727,
            Feature::Status => 7725,
            Feature::Progress => 7720,
            Feature::Tab => 7719,
            Feature::Badge => 7718,
            Feature::Mark => 7722,
            Feature::Key => 7723,
            Feature::Palette => 7724,
            Feature::Fold => 7721,
            Feature::Link => 7726,
        }
    }

    pub fn from_number(n: u16) -> Option<Feature> {
        Feature::ALL.into_iter().find(|f| f.number() == n)
    }

    pub fn from_name(s: &str) -> Option<Feature> {
        Feature::ALL.into_iter().find(|f| f.name() == s)
    }

    /// Protocol version of the sequence, `0` while it is not built yet.
    pub fn version(self) -> u8 {
        match self {
            Feature::Menu
            | Feature::Caps
            | Feature::Status
            | Feature::Progress
            | Feature::Tab
            | Feature::Badge
            | Feature::Mark
            | Feature::Key
            | Feature::Palette
            | Feature::Fold
            | Feature::Link => 1,
        }
    }

    fn index(self) -> usize {
        Feature::ALL.iter().position(|f| *f == self).unwrap_or(0)
    }
}

/// `[private_osc.features]`: one switch per sequence name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features([bool; 11]);

impl Default for Features {
    fn default() -> Self {
        Features([true; 11])
    }
}

impl Features {
    pub fn get(&self, f: Feature) -> bool {
        self.0[f.index()]
    }

    pub fn set(&mut self, f: Feature, on: bool) {
        self.0[f.index()] = on;
    }
}

/// The two layers of §6.4.2, as one value. This is the only place that reads them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrivateGate {
    /// Layer 1: the absolute master switch (config, possibly overridden for the session).
    pub enabled: bool,
    /// Layer 2: per-feature switches.
    pub features: Features,
    /// Stateful sequences per pane and second (2–100).
    pub rate: u32,
    /// Whether `7727` may be answered. Off in the daemon, which has no program to talk to.
    pub replies: bool,
    /// `fold_commands`: OSC 133 command output is the default fold unit (SPEC §6.4.12).
    pub fold_commands: bool,
}

impl Default for PrivateGate {
    /// Fail closed.
    fn default() -> Self {
        PrivateGate {
            enabled: false,
            features: Features::default(),
            rate: DEFAULT_RATE,
            replies: true,
            fold_commands: false,
        }
    }
}

impl PrivateGate {
    /// Both layers allow `f`.
    pub fn allows(&self, f: Feature) -> bool {
        self.enabled && self.features.get(f)
    }

    /// The daemon's gate: stores state for replay, never replies (the client's gate decides
    /// what is shown, because the client parses the replayed bytes).
    pub fn store_only() -> Self {
        PrivateGate {
            enabled: true,
            replies: false,
            ..Default::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Ok,
    Warn,
    Error,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Ok => "ok",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }

    fn parse(s: &str) -> Option<Level> {
        match s {
            "info" => Some(Level::Info),
            "ok" => Some(Level::Ok),
            "warn" => Some(Level::Warn),
            "error" => Some(Level::Error),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub slot: String,
    pub text: String,
    pub level: Level,
    pub tooltip: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusCmd {
    Set { seg: Segment, replay: bool },
    Clear(Option<String>),
}

/// One `7720` bar. `pct` is `None` while indeterminate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bar {
    pub id: String,
    pub pct: Option<u8>,
    pub label: String,
    pub eta_s: Option<u32>,
    /// The bar was marked `done` (it shows 100 %).
    pub done: bool,
    updated: u64,
    done_at: u64,
}

impl Bar {
    fn same(&self, o: &Bar) -> bool {
        (&self.id, self.pct, &self.label, self.eta_s, self.done)
            == (&o.id, o.pct, &o.label, o.eta_s, o.done)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProgressCmd {
    Set { bar: Bar, replay: bool },
    Done(String),
    Clear(Option<String>),
}

/// `7719;tab;color;<value>`: `value` is empty (clear), a color or palette name, or `#rrggbb`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabCmd {
    pub value: String,
    pub replay: bool,
}

/// The theme token of a `7718` badge: tokens only, never an arbitrary color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BadgeToken {
    #[default]
    Muted,
    Accent,
    Success,
    Warning,
    Error,
}

impl BadgeToken {
    pub fn name(self) -> &'static str {
        match self {
            BadgeToken::Muted => "muted",
            BadgeToken::Accent => "accent",
            BadgeToken::Success => "success",
            BadgeToken::Warning => "warning",
            BadgeToken::Error => "error",
        }
    }

    fn parse(s: &str) -> Option<BadgeToken> {
        Some(match s {
            "muted" => BadgeToken::Muted,
            "accent" => BadgeToken::Accent,
            "success" => BadgeToken::Success,
            "warning" => BadgeToken::Warning,
            "error" => BadgeToken::Error,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Badge {
    pub text: String,
    pub token: BadgeToken,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BadgeCmd {
    Set { badge: Badge, replay: bool },
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    Info,
    Warn,
    Error,
}

impl MarkKind {
    pub fn name(self) -> &'static str {
        match self {
            MarkKind::Info => "info",
            MarkKind::Warn => "warn",
            MarkKind::Error => "error",
        }
    }

    fn parse(s: &str) -> Option<MarkKind> {
        match s {
            "info" => Some(MarkKind::Info),
            "warn" => Some(MarkKind::Warn),
            "error" => Some(MarkKind::Error),
            _ => None,
        }
    }
}

/// One `7722` mark, pinned to a stable line id (`trimmed + absolute row`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramMark {
    pub line: u64,
    pub kind: MarkKind,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkCmd {
    Add {
        kind: MarkKind,
        note: String,
        replay: bool,
    },
    Clear,
}

/// One decoded 77xx sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrivCmd {
    Menu(RegCmd),
    CapsQuery,
    Status(StatusCmd),
    Progress(ProgressCmd),
    Tab(TabCmd),
    Badge(BadgeCmd),
    Mark(MarkCmd),
    Key(KeyCmd),
    Palette(PaletteCmd),
    Fold(FoldCmd),
    Link(LinkCmd),
    /// Allocated but not built yet, or an unallocated number: nothing to do once it is allowed.
    Reserved(Option<Feature>),
    /// Namespace mismatch, bad fields or an oversized string; `None` when the number is unknown.
    Malformed(Option<Feature>),
}

impl PrivCmd {
    fn feature(&self) -> Option<Feature> {
        match self {
            PrivCmd::Menu(_) => Some(Feature::Menu),
            PrivCmd::CapsQuery => Some(Feature::Caps),
            PrivCmd::Status(_) => Some(Feature::Status),
            PrivCmd::Progress(_) => Some(Feature::Progress),
            PrivCmd::Tab(_) => Some(Feature::Tab),
            PrivCmd::Badge(_) => Some(Feature::Badge),
            PrivCmd::Mark(_) => Some(Feature::Mark),
            PrivCmd::Key(_) => Some(Feature::Key),
            PrivCmd::Palette(_) => Some(Feature::Palette),
            PrivCmd::Fold(_) => Some(Feature::Fold),
            PrivCmd::Link(_) => Some(Feature::Link),
            PrivCmd::Reserved(f) | PrivCmd::Malformed(f) => *f,
        }
    }
}

/// Whether an OSC payload (after `ESC ]`) starts with a number of the private block.
pub fn is_private(p: &[u8]) -> bool {
    p.len() >= 4
        && p.starts_with(b"77")
        && p[2].is_ascii_digit()
        && p[3].is_ascii_digit()
        && (p.len() == 4 || p[4] == b';')
}

/// The feature a payload addresses (by number), for an oversized string.
pub fn feature_of_payload(p: &[u8]) -> Option<Feature> {
    let n = std::str::from_utf8(p.get(..4)?).ok()?.parse::<u16>().ok()?;
    Feature::from_number(n)
}

/// Decodes a payload of the block; `None` when it is not in `7700–7799`.
pub fn parse(p: &[u8]) -> Option<PrivCmd> {
    if !is_private(p) {
        return None;
    }
    let feature = feature_of_payload(p);
    let Some(f) = feature else {
        return Some(PrivCmd::Reserved(None));
    };
    if p.len() > MAX_OSC {
        return Some(PrivCmd::Malformed(Some(f)));
    }
    let rest = p.get(5..).unwrap_or(&[]);
    let Some(rest) = rest.strip_prefix(f.name().as_bytes()) else {
        return Some(PrivCmd::Malformed(Some(f)));
    };
    // `rest` is empty or `;<verb>…`.
    let verbs = match rest.strip_prefix(b";") {
        Some(v) => v,
        None if rest.is_empty() => &[][..],
        None => return Some(PrivCmd::Malformed(Some(f))),
    };
    let bad = || Some(PrivCmd::Malformed(Some(f)));
    Some(match f {
        Feature::Menu => {
            let mut full = b"menu;".to_vec();
            full.extend_from_slice(verbs);
            match menu_reg::parse(&full) {
                Some(RegCmd::Malformed) | None => return bad(),
                Some(c) => PrivCmd::Menu(c),
            }
        }
        Feature::Caps => {
            if verbs == b"query" {
                PrivCmd::CapsQuery
            } else {
                return bad();
            }
        }
        Feature::Status => match parse_status(verbs) {
            Some(c) => PrivCmd::Status(c),
            None => return bad(),
        },
        Feature::Progress => match parse_progress(verbs) {
            Some(c) => PrivCmd::Progress(c),
            None => return bad(),
        },
        Feature::Tab => match parse_tab(verbs) {
            Some(c) => PrivCmd::Tab(c),
            None => return bad(),
        },
        Feature::Badge => match parse_badge(verbs) {
            Some(c) => PrivCmd::Badge(c),
            None => return bad(),
        },
        Feature::Mark => match parse_mark(verbs) {
            Some(c) => PrivCmd::Mark(c),
            None => return bad(),
        },
        Feature::Key => match parse_key(verbs) {
            Some(c) => PrivCmd::Key(c),
            None => return bad(),
        },
        Feature::Palette => match parse_palette(verbs) {
            Some(c) => PrivCmd::Palette(c),
            None => return bad(),
        },
        Feature::Fold => match parse_fold(verbs) {
            Some(c) => PrivCmd::Fold(c),
            None => return bad(),
        },
        Feature::Link => match parse_link(verbs) {
            Some(c) => PrivCmd::Link(c),
            None => return bad(),
        },
    })
}

fn valid_id(s: &str) -> bool {
    valid_slot(s)
}

fn valid_slot(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// The shared sanitiser (§12.4): controls, C1 and bidi characters removed, whitespace
/// collapsed, plain text only.
pub fn sanitize(s: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if c.is_control() || is_bidi(c) {
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(c);
    }
    out
}

fn is_bidi(c: char) -> bool {
    matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

fn decode_text(b64: &str) -> Option<String> {
    String::from_utf8(b64_decode(b64)?).ok()
}

fn parse_status(verbs: &[u8]) -> Option<StatusCmd> {
    let s = std::str::from_utf8(verbs).ok()?;
    let mut f = s.split(';');
    match f.next()? {
        "set" => {
            let fields: Vec<&str> = f.collect();
            let replay = match fields.len() {
                2..=4 => false,
                5 if fields[4] == "replay" => true,
                _ => return None,
            };
            if !valid_slot(fields[0]) {
                return None;
            }
            let text = sanitize(&decode_text(fields[1])?);
            if text.is_empty() || text.chars().count() > STATUS_TEXT_MAX {
                return None;
            }
            let level = match fields.get(2) {
                None => Level::Info,
                Some(l) => Level::parse(l)?,
            };
            let tooltip = match fields.get(3) {
                None | Some(&"") => String::new(),
                Some(t) => {
                    let raw = b64_decode(t)?;
                    if raw.len() > STATUS_TOOLTIP_MAX {
                        return None;
                    }
                    sanitize(&String::from_utf8(raw).ok()?)
                }
            };
            Some(StatusCmd::Set {
                seg: Segment {
                    slot: fields[0].to_string(),
                    text,
                    level,
                    tooltip,
                },
                replay,
            })
        }
        "clear" => match (f.next(), f.next()) {
            (None, _) | (Some(""), None) => Some(StatusCmd::Clear(None)),
            (Some(slot), None) if valid_slot(slot) => Some(StatusCmd::Clear(Some(slot.into()))),
            _ => None,
        },
        _ => None,
    }
}

fn parse_progress(verbs: &[u8]) -> Option<ProgressCmd> {
    let s = std::str::from_utf8(verbs).ok()?;
    let mut f = s.split(';');
    match f.next()? {
        "set" => {
            let fields: Vec<&str> = f.collect();
            let replay = match fields.len() {
                3 | 4 => false,
                5 if fields[4] == "replay" => true,
                _ => return None,
            };
            if !valid_id(fields[0]) {
                return None;
            }
            let pct = match fields[1] {
                "-" => None,
                p if !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit()) => {
                    let n: u16 = p.parse().ok()?;
                    if n > 100 {
                        return None;
                    }
                    Some(n as u8)
                }
                _ => return None,
            };
            let label = sanitize(&decode_text(fields[2])?);
            if label.is_empty() || label.chars().count() > PROGRESS_LABEL_MAX {
                return None;
            }
            let eta_s = match fields.get(3) {
                None => None,
                Some(&"") if replay => None,
                Some(e)
                    if !e.is_empty() && e.len() <= 6 && e.bytes().all(|b| b.is_ascii_digit()) =>
                {
                    let n: u32 = e.parse().ok()?;
                    if n > ETA_MAX {
                        return None;
                    }
                    Some(n)
                }
                _ => return None,
            };
            Some(ProgressCmd::Set {
                bar: Bar {
                    id: fields[0].to_string(),
                    pct,
                    label,
                    eta_s,
                    done: false,
                    updated: 0,
                    done_at: 0,
                },
                replay,
            })
        }
        "done" => match (f.next(), f.next()) {
            (Some(id), None) if valid_id(id) => Some(ProgressCmd::Done(id.into())),
            _ => None,
        },
        "clear" => match (f.next(), f.next()) {
            (None, _) | (Some(""), None) => Some(ProgressCmd::Clear(None)),
            (Some(id), None) if valid_id(id) => Some(ProgressCmd::Clear(Some(id.into()))),
            _ => None,
        },
        _ => None,
    }
}

/// Whether `v` is something the M11.3 color parser may accept: a name or `#rrggbb`.
fn valid_color_syntax(v: &str) -> bool {
    match v.strip_prefix('#') {
        Some(h) => h.len() == 6 && h.bytes().all(|b| b.is_ascii_hexdigit()),
        None => valid_slot(v),
    }
}

fn parse_tab(verbs: &[u8]) -> Option<TabCmd> {
    let s = std::str::from_utf8(verbs).ok()?;
    let fields: Vec<&str> = s.split(';').collect();
    if fields.first() != Some(&"color") {
        return None;
    }
    let (value, replay) = match fields.len() {
        2 => (fields[1], false),
        3 if fields[2] == "replay" => (fields[1], true),
        _ => return None,
    };
    if !value.is_empty() && !valid_color_syntax(value) {
        return None;
    }
    Some(TabCmd {
        value: value.to_string(),
        replay,
    })
}

fn parse_badge(verbs: &[u8]) -> Option<BadgeCmd> {
    let s = std::str::from_utf8(verbs).ok()?;
    let mut f = s.split(';');
    match f.next()? {
        "set" => {
            let fields: Vec<&str> = f.collect();
            let replay = match fields.len() {
                1 | 2 => false,
                3 if fields[2] == "replay" => true,
                _ => return None,
            };
            let text = sanitize(&decode_text(fields[0])?);
            if text.is_empty() || text.chars().count() > BADGE_TEXT_MAX {
                return None;
            }
            let token = match fields.get(1) {
                None => BadgeToken::Muted,
                Some(t) => BadgeToken::parse(t)?,
            };
            Some(BadgeCmd::Set {
                badge: Badge { text, token },
                replay,
            })
        }
        "clear" if f.next().is_none() => Some(BadgeCmd::Clear),
        _ => None,
    }
}

fn parse_mark(verbs: &[u8]) -> Option<MarkCmd> {
    let s = std::str::from_utf8(verbs).ok()?;
    let fields: Vec<&str> = s.split(';').collect();
    match fields.as_slice() {
        ["clear"] => Some(MarkCmd::Clear),
        ["add", kind, note, rest @ ..] => {
            let replay = match rest {
                [] => false,
                ["replay"] => true,
                _ => return None,
            };
            let kind = MarkKind::parse(kind)?;
            let note = sanitize(&decode_text(note)?);
            if note.chars().count() > MARK_NOTE_MAX {
                return None;
            }
            Some(MarkCmd::Add { kind, note, replay })
        }
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FoldCmd {
    Begin {
        id: String,
        title: String,
        replay: bool,
    },
    End {
        id: String,
        replay: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkCmd {
    Add {
        id: String,
        regex: String,
        template: String,
        replay: bool,
    },
    Clear(Option<String>),
}

fn replay_flag(rest: &[&str]) -> Option<bool> {
    match rest {
        [] => Some(false),
        ["replay"] => Some(true),
        _ => None,
    }
}

fn parse_fold(verbs: &[u8]) -> Option<FoldCmd> {
    let s = std::str::from_utf8(verbs).ok()?;
    let fields: Vec<&str> = s.split(';').collect();
    match fields.as_slice() {
        ["begin", id, title, rest @ ..] => {
            let replay = replay_flag(rest)?;
            if !valid_slot(id) {
                return None;
            }
            let title = sanitize(&decode_text(title)?);
            if title.is_empty() || title.chars().count() > FOLD_TITLE_MAX {
                return None;
            }
            Some(FoldCmd::Begin {
                id: (*id).to_string(),
                title,
                replay,
            })
        }
        ["end", id, rest @ ..] => {
            let replay = replay_flag(rest)?;
            valid_slot(id).then(|| FoldCmd::End {
                id: (*id).to_string(),
                replay,
            })
        }
        _ => None,
    }
}

fn parse_link(verbs: &[u8]) -> Option<LinkCmd> {
    let s = std::str::from_utf8(verbs).ok()?;
    let fields: Vec<&str> = s.split(';').collect();
    match fields.as_slice() {
        ["clear", rest @ ..] => parse_clear(rest).map(LinkCmd::Clear),
        ["add", id, regex, template, rest @ ..] => {
            let replay = replay_flag(rest)?;
            if !valid_slot(id) {
                return None;
            }
            let regex = decode_text(regex)?;
            let template = decode_text(template)?;
            if regex.is_empty()
                || regex.len() > LINK_REGEX_MAX
                || template.is_empty()
                || template.len() > LINK_TEMPLATE_MAX
                || template.chars().any(|c| c.is_control() || is_bidi(c))
            {
                return None;
            }
            // Same non-backtracking engine and complexity cap as `[[hyperlink_rules]]`:
            // lookaround, backreferences and expensive patterns do not compile.
            mt_patterns::compile_capped(&regex).ok()?;
            Some(LinkCmd::Add {
                id: (*id).to_string(),
                regex,
                template,
                replay,
            })
        }
        _ => None,
    }
}

/// One `7721` region. Lines are stable ids (`MarkStore` semantics); `end` is exclusive and
/// `None` while the region is open (an open region runs to the last row).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldRegion {
    pub id: String,
    pub title: String,
    pub start: u64,
    pub end: Option<u64>,
    pub depth: u8,
}

/// The `7721` regions of one pane, oldest first.
#[derive(Clone, Debug, Default)]
pub struct FoldStore {
    regions: std::collections::VecDeque<FoldRegion>,
    rate: RateWindow,
}

impl FoldStore {
    pub fn regions(&self) -> Vec<FoldRegion> {
        self.regions.iter().cloned().collect()
    }

    /// Drops regions that left the history (`line < base`).
    pub fn prune(&mut self, base: u64) -> bool {
        let n = self.regions.len();
        self.regions
            .retain(|r| r.end.map_or(r.start >= base, |e| e > base));
        n != self.regions.len()
    }

    /// `Err(())` rate-dropped; `Ok(None)` malformed (alternate screen, unmatched `end`,
    /// overlap, depth, duplicate open id).
    fn apply(&mut self, cmd: FoldCmd, line: Option<u64>, now_ms: u64) -> Result<Option<bool>, ()> {
        let replay = matches!(
            cmd,
            FoldCmd::Begin { replay: true, .. } | FoldCmd::End { replay: true, .. }
        );
        if !replay && !self.rate.admit(now_ms, FOLD_RATE_PER_SEC) {
            return Err(());
        }
        let Some(line) = line else {
            return Ok(None);
        };
        Ok(match cmd {
            FoldCmd::Begin { id, title, .. } => {
                let open = self.regions.iter().filter(|r| r.end.is_none());
                if open.clone().any(|r| r.id == id) || open.count() >= FOLD_DEPTH_MAX {
                    return Ok(None);
                }
                let depth = self.regions.iter().filter(|r| r.end.is_none()).count() as u8;
                if self.regions.len() >= FOLD_REGIONS_MAX {
                    self.regions.pop_front();
                }
                self.regions.push_back(FoldRegion {
                    id,
                    title,
                    start: line,
                    end: None,
                    depth,
                });
                Some(true)
            }
            FoldCmd::End { id, .. } => {
                // Only the innermost open region may close (non-nested ids overlap).
                let Some(innermost) = self.regions.iter_mut().rev().find(|r| r.end.is_none())
                else {
                    return Ok(None);
                };
                if innermost.id != id || line < innermost.start {
                    return Ok(None);
                }
                innermost.end = Some(line);
                Some(true)
            }
        })
    }

    /// `StateOwner::clear` for callers that do not import the trait.
    pub fn clear_all(&mut self) -> bool {
        StateOwner::clear(self)
    }
}

impl StateOwner for FoldStore {
    fn clear(&mut self) -> bool {
        let changed = !self.regions.is_empty();
        self.regions.clear();
        changed
    }
}

/// One `7726` rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkRule {
    pub id: String,
    pub regex: String,
    pub template: String,
}

/// The `7726` rules of one pane, in registration order.
#[derive(Clone, Debug, Default)]
pub struct LinkRules {
    rules: Vec<LinkRule>,
    rev: u64,
    rate: RateWindow,
}

impl LinkRules {
    pub fn rules(&self) -> &[LinkRule] {
        &self.rules
    }

    /// Bumped on every change, so a matcher cache can key on it.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// `Err(())` rate-dropped, `Ok(None)` malformed (cap reached).
    fn apply(&mut self, cmd: LinkCmd, now_ms: u64) -> Result<Option<bool>, ()> {
        let replay = matches!(cmd, LinkCmd::Add { replay: true, .. });
        if !replay && !self.rate.admit(now_ms, LINK_RATE_PER_SEC) {
            return Err(());
        }
        let changed = match cmd {
            LinkCmd::Clear(None) => StateOwner::clear(self),
            LinkCmd::Clear(Some(id)) => {
                let n = self.rules.len();
                self.rules.retain(|r| r.id != id);
                n != self.rules.len()
            }
            LinkCmd::Add {
                id,
                regex,
                template,
                ..
            } => {
                let new = LinkRule {
                    id,
                    regex,
                    template,
                };
                if let Some(cur) = self.rules.iter_mut().find(|r| r.id == new.id) {
                    let c = *cur != new;
                    *cur = new;
                    c
                } else if self.rules.len() >= LINK_RULES_MAX {
                    return Ok(None);
                } else {
                    self.rules.push(new);
                    true
                }
            }
        };
        if changed {
            self.rev += 1;
        }
        Ok(Some(changed))
    }

    pub(crate) fn replay(&self, out: &mut String) {
        for r in &self.rules {
            let _ = write!(
                out,
                "\x1b]7726;link;add;{};{};{};replay\x07",
                r.id,
                b64_encode(r.regex.as_bytes()),
                b64_encode(r.template.as_bytes())
            );
        }
    }
}

impl StateOwner for LinkRules {
    fn clear(&mut self) -> bool {
        let changed = !self.rules.is_empty();
        self.rules.clear();
        if changed {
            self.rev += 1;
        }
        changed
    }
}

/// What a pane-registered item does (`spawn` stays a `7717` privilege).
pub use crate::menu_reg::RegKind as PaneKind;

/// One `7723` binding: a canonical chord (`ctrl-alt-t`) and what it does in its own pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneKey {
    pub id: String,
    pub chord: String,
    pub kind: PaneKind,
    pub payload: String,
}

/// One `7724` palette entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneEntry {
    pub id: String,
    pub label: String,
    pub kind: PaneKind,
    pub payload: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyCmd {
    Add { key: PaneKey, replay: bool },
    Clear(Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteCmd {
    Add { entry: PaneEntry, replay: bool },
    Clear(Option<String>),
}

static TAKEN_CHORDS: std::sync::RwLock<Vec<String>> = std::sync::RwLock::new(Vec::new());

/// Publishes the canonical chords the live keymap (user `[[keys]]` and defaults) uses, so a
/// pane binding on one of them is refused as malformed. Process-wide: one keymap per GUI.
pub fn set_taken_chords(chords: impl IntoIterator<Item = String>) {
    let mut v: Vec<String> = chords
        .into_iter()
        .filter_map(|c| canonical_chord(&c))
        .collect();
    v.sort();
    v.dedup();
    *TAKEN_CHORDS.write().unwrap_or_else(|e| e.into_inner()) = v;
}

fn is_taken(chord: &str) -> bool {
    TAKEN_CHORDS
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|c| c == chord)
}

/// Chords that close, quit, copy, paste, open the palette or toggle the master switch whatever
/// the user's keymap says (SPEC §6.4.11a).
const RESERVED_CHORDS: &[&str] = &[
    "cmd-q",
    "cmd-w",
    "shift-cmd-w",
    "cmd-c",
    "cmd-v",
    "shift-cmd-p",
    "shift-cmd-c",
    "shift-cmd-v",
    "shift-cmd-q",
    "ctrl-shift-q",
    "ctrl-shift-w",
    "ctrl-shift-c",
    "ctrl-shift-v",
    "ctrl-shift-p",
    "ctrl-shift-x",
    "alt-f4",
    "ctrl-insert",
    "shift-insert",
    "ctrl-q",
    "ctrl-w",
];

/// Parses `ctrl+alt+t` (§9.1 syntax) into the canonical `ctrl-alt-shift-cmd-key` form. `None`
/// for bad syntax; this mirrors `mt_config::keys::Keystroke`, which `mt_term` cannot depend on.
pub fn canonical_chord(s: &str) -> Option<String> {
    let s = s.trim();
    let (mods, key) = if s == "+" {
        ("", "+")
    } else if let Some(m) = s.strip_suffix("++") {
        (m, "+")
    } else {
        s.rsplit_once('+').unwrap_or(("", s))
    };
    let (mut cmd, mut ctrl, mut alt, mut shift) = (false, false, false, false);
    if !mods.is_empty() {
        for m in mods.split('+') {
            let flag = match m.trim().to_ascii_lowercase().as_str() {
                "cmd" | "super" => &mut cmd,
                "ctrl" | "control" => &mut ctrl,
                "alt" | "opt" | "option" => &mut alt,
                "shift" => &mut shift,
                _ => return None,
            };
            if *flag {
                return None;
            }
            *flag = true;
        }
    }
    let key = key.to_ascii_lowercase();
    let named = [
        "enter",
        "tab",
        "space",
        "escape",
        "backspace",
        "delete",
        "insert",
        "home",
        "end",
        "pageup",
        "pagedown",
        "up",
        "down",
        "left",
        "right",
    ];
    let function = key
        .strip_prefix('f')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=24).contains(&n));
    if !(key.chars().count() == 1 || named.contains(&key.as_str()) || function) {
        return None;
    }
    // A modifier (not shift alone) or a function key is required.
    if !(ctrl || alt || cmd || function) {
        return None;
    }
    let mut out = String::new();
    for (on, name) in [(ctrl, "ctrl"), (alt, "alt"), (shift, "shift"), (cmd, "cmd")] {
        if on {
            out.push_str(name);
            out.push('-');
        }
    }
    out.push_str(&key);
    Some(out)
}

/// Canonical `ctrl-alt-t` back to the `+` syntax of §9.1 (`ctrl+alt+t`; the key may be `-`).
fn chord_syntax(chord: &str) -> String {
    let mut rest = chord;
    let mut parts: Vec<&str> = Vec::new();
    while let Some((m, tail)) = ["ctrl-", "alt-", "shift-", "cmd-"]
        .iter()
        .find_map(|m| rest.strip_prefix(m).map(|t| (m.trim_end_matches('-'), t)))
    {
        if tail.is_empty() {
            break;
        }
        parts.push(m);
        rest = tail;
    }
    parts.push(rest);
    parts.join("+")
}

/// Like [`set_taken_chords`] for GPUI keystroke strings (`cmd-shift-p`).
pub fn set_taken_gpui_chords<'a>(chords: impl IntoIterator<Item = &'a str>) {
    set_taken_chords(chords.into_iter().map(chord_syntax));
}

fn parse_pane_kind(s: &str) -> Option<PaneKind> {
    match s {
        "send" => Some(PaneKind::Send),
        "ctl" => Some(PaneKind::Ctl),
        _ => None,
    }
}

fn pane_payload(b64: &str) -> Option<String> {
    let p = String::from_utf8(b64_decode(b64)?).ok()?;
    (!p.is_empty() && p.len() <= menu_reg::MAX_PAYLOAD).then_some(p)
}

fn parse_clear(f: &[&str]) -> Option<Option<String>> {
    match f {
        [] | [""] => Some(None),
        [id] if valid_slot(id) => Some(Some((*id).to_string())),
        _ => None,
    }
}

fn parse_key(verbs: &[u8]) -> Option<KeyCmd> {
    let s = std::str::from_utf8(verbs).ok()?;
    let fields: Vec<&str> = s.split(';').collect();
    match fields.as_slice() {
        ["clear", rest @ ..] => parse_clear(rest).map(KeyCmd::Clear),
        ["add", id, chord, kind, payload, rest @ ..] => {
            let replay = match rest {
                [] => false,
                ["replay"] => true,
                _ => return None,
            };
            if !valid_slot(id) {
                return None;
            }
            Some(KeyCmd::Add {
                key: PaneKey {
                    id: (*id).to_string(),
                    chord: canonical_chord(chord)?,
                    kind: parse_pane_kind(kind)?,
                    payload: pane_payload(payload)?,
                },
                replay,
            })
        }
        _ => None,
    }
}

fn parse_palette(verbs: &[u8]) -> Option<PaletteCmd> {
    let s = std::str::from_utf8(verbs).ok()?;
    let fields: Vec<&str> = s.split(';').collect();
    match fields.as_slice() {
        ["clear", rest @ ..] => parse_clear(rest).map(PaletteCmd::Clear),
        ["add", id, label, kind, payload, rest @ ..] => {
            let replay = match rest {
                [] => false,
                ["replay"] => true,
                _ => return None,
            };
            if !valid_slot(id) {
                return None;
            }
            let label: String = String::from_utf8(b64_decode(label)?)
                .ok()?
                .chars()
                .filter(|c| !c.is_control() && !is_bidi(*c))
                .collect::<String>()
                .trim()
                .to_string();
            if label.is_empty() || label.len() > menu_reg::MAX_LABEL {
                return None;
            }
            Some(PaletteCmd::Add {
                entry: PaneEntry {
                    id: (*id).to_string(),
                    label,
                    kind: parse_pane_kind(kind)?,
                    payload: pane_payload(payload)?,
                },
                replay,
            })
        }
        _ => None,
    }
}

/// A one-second rate window shared by the `7718` and `7722` stores.
#[derive(Clone, Copy, Debug, Default)]
struct RateWindow {
    start: u64,
    count: u32,
}

impl RateWindow {
    /// Whether one more command fits into the window ending `WINDOW_MS` after its first.
    fn admit(&mut self, now_ms: u64, limit: u32) -> bool {
        if now_ms.saturating_sub(self.start) >= WINDOW_MS || self.count == 0 {
            self.start = now_ms;
            self.count = 0;
        }
        if self.count >= limit {
            return false;
        }
        self.count += 1;
        true
    }
}

/// State a feature keeps for one pane; cleared when the master switch or its gate goes off, on
/// RIS and when the pane's process exits. Later milestones register more of these.
pub trait StateOwner {
    /// Drops everything. Returns whether anything existed.
    fn clear(&mut self) -> bool;
}

impl StateOwner for MenuRegistry {
    fn clear(&mut self) -> bool {
        self.reset()
    }
}

/// The `7725` segments of one pane.
#[derive(Clone, Debug, Default)]
pub struct StatusSegments {
    segs: BTreeMap<String, Segment>,
    window_start: u64,
    window_count: u32,
}

impl StatusSegments {
    /// Segments in slot-name order.
    pub fn segments(&self) -> Vec<Segment> {
        self.segs.values().cloned().collect()
    }

    /// Returns `Err(())` when the rate window dropped the command, else whether anything changed.
    fn apply(&mut self, cmd: StatusCmd, now_ms: u64) -> Result<bool, ()> {
        let replay = matches!(cmd, StatusCmd::Set { replay: true, .. });
        if !replay {
            if now_ms.saturating_sub(self.window_start) >= WINDOW_MS || self.window_count == 0 {
                self.window_start = now_ms;
                self.window_count = 0;
            }
            if self.window_count >= STATUS_RATE_PER_SEC {
                return Err(());
            }
            self.window_count += 1;
        }
        Ok(match cmd {
            StatusCmd::Set { seg, .. } => {
                if let Some(cur) = self.segs.get_mut(&seg.slot) {
                    let changed = *cur != seg;
                    *cur = seg;
                    changed
                } else if self.segs.len() >= STATUS_SLOTS {
                    return Ok(false);
                } else {
                    self.segs.insert(seg.slot.clone(), seg);
                    true
                }
            }
            StatusCmd::Clear(None) => self.clear(),
            StatusCmd::Clear(Some(slot)) => self.segs.remove(&slot).is_some(),
        })
    }

    /// The sequences that rebuild the segments, for the daemon's replay stream.
    pub fn replay(&self, out: &mut String) {
        for s in self.segs.values() {
            let _ = write!(
                out,
                "\x1b]7725;status;set;{};{};{};{};replay\x07",
                s.slot,
                b64_encode(s.text.as_bytes()),
                s.level.name(),
                b64_encode(s.tooltip.as_bytes())
            );
        }
    }
}

impl StateOwner for StatusSegments {
    fn clear(&mut self) -> bool {
        let changed = !self.segs.is_empty();
        self.segs.clear();
        changed
    }
}

/// The `7720` bars of one pane.
#[derive(Clone, Debug, Default)]
pub struct ProgressBars {
    bars: BTreeMap<String, Bar>,
    /// Throttled `set`s: the newest value per id, applied when the window rolls.
    pending: BTreeMap<String, Bar>,
    window_start: u64,
    window_count: u32,
}

impl ProgressBars {
    /// Bars in id order; a `done` bar reads 100 %.
    pub fn bars(&self) -> Vec<Bar> {
        self.bars
            .values()
            .map(|b| {
                let mut b = b.clone();
                if b.done {
                    b.pct = Some(100);
                    b.eta_s = None;
                }
                b
            })
            .collect()
    }

    fn take_slot(&mut self, now: u64) -> bool {
        if now.saturating_sub(self.window_start) >= WINDOW_MS || self.window_count == 0 {
            self.window_start = now;
            self.window_count = 0;
        }
        if self.window_count >= PROGRESS_RATE_PER_SEC {
            return false;
        }
        self.window_count += 1;
        true
    }

    fn put(&mut self, mut bar: Bar, now: u64) -> bool {
        bar.updated = now;
        if let Some(cur) = self.bars.get_mut(&bar.id) {
            let changed = !cur.same(&bar);
            *cur = bar;
            return changed;
        }
        if self.bars.len() >= PROGRESS_BARS {
            return false;
        }
        self.bars.insert(bar.id.clone(), bar);
        true
    }

    /// Applies the throttled `set`s once the window has rolled.
    fn flush(&mut self, now: u64) -> bool {
        if self.pending.is_empty() || now.saturating_sub(self.window_start) < WINDOW_MS {
            return false;
        }
        let mut changed = false;
        for (_, bar) in std::mem::take(&mut self.pending) {
            if self.take_slot(now) {
                changed |= self.put(bar, now);
            }
        }
        changed
    }

    /// Returns whether anything changed.
    fn apply(&mut self, cmd: ProgressCmd, now: u64) -> bool {
        let mut changed = self.flush(now);
        match cmd {
            ProgressCmd::Set { bar, replay } => {
                if replay {
                    changed |= self.put(bar, now);
                } else if self.take_slot(now) {
                    self.pending.remove(&bar.id);
                    changed |= self.put(bar, now);
                } else if self.pending.contains_key(&bar.id) || self.pending.len() < PROGRESS_BARS {
                    self.pending.insert(bar.id.clone(), bar);
                }
            }
            ProgressCmd::Done(id) => {
                self.pending.remove(&id);
                if let Some(b) = self.bars.get_mut(&id) {
                    if !b.done {
                        b.done = true;
                        b.done_at = now;
                        changed = true;
                    }
                }
            }
            ProgressCmd::Clear(None) => {
                self.pending.clear();
                changed |= self.clear();
            }
            ProgressCmd::Clear(Some(id)) => {
                self.pending.remove(&id);
                changed |= self.bars.remove(&id).is_some();
            }
        }
        changed
    }

    /// Drops stale and finished bars, applies throttled `set`s. Returns whether anything changed.
    pub fn expire(&mut self, now: u64) -> bool {
        let mut changed = self.flush(now);
        let before = self.bars.len();
        self.bars.retain(|_, b| {
            if b.done {
                now.saturating_sub(b.done_at) < PROGRESS_DONE_MS
            } else {
                now.saturating_sub(b.updated) < PROGRESS_STALE_MS
            }
        });
        changed |= self.bars.len() != before;
        changed
    }

    /// The sequences that rebuild the bars, for the daemon's replay stream.
    pub fn replay(&self, out: &mut String) {
        for b in self.bars.values().filter(|b| !b.done) {
            let _ = write!(
                out,
                "\x1b]7720;progress;set;{};{};{};{};replay\x07",
                b.id,
                b.pct.map_or("-".to_string(), |p| p.to_string()),
                b64_encode(b.label.as_bytes()),
                b.eta_s.map_or(String::new(), |e| e.to_string())
            );
        }
    }
}

impl StateOwner for ProgressBars {
    fn clear(&mut self) -> bool {
        self.pending.clear();
        let changed = !self.bars.is_empty();
        self.bars.clear();
        changed
    }
}

/// The color a program declared for its tab (`7719`), as the string it sent.
#[derive(Clone, Debug, Default)]
pub struct DeclaredTab {
    color: Option<String>,
}

impl DeclaredTab {
    pub fn color(&self) -> Option<&str> {
        self.color.as_deref()
    }

    fn apply(&mut self, cmd: TabCmd) -> bool {
        let new = (!cmd.value.is_empty()).then_some(cmd.value);
        let changed = self.color != new;
        self.color = new;
        changed
    }

    pub fn replay(&self, out: &mut String) {
        if let Some(c) = &self.color {
            let _ = write!(out, "\x1b]7719;tab;color;{c};replay\x07");
        }
    }
}

impl StateOwner for DeclaredTab {
    fn clear(&mut self) -> bool {
        self.color.take().is_some()
    }
}

/// The `7718` badge of one pane.
#[derive(Clone, Debug, Default)]
pub struct BadgeState {
    badge: Option<Badge>,
    rate: RateWindow,
}

impl BadgeState {
    pub fn badge(&self) -> Option<&Badge> {
        self.badge.as_ref()
    }

    /// `Err(())` when the rate window dropped the command, else whether anything changed.
    fn apply(&mut self, cmd: BadgeCmd, now_ms: u64) -> Result<bool, ()> {
        let replay = matches!(cmd, BadgeCmd::Set { replay: true, .. });
        if !replay && !self.rate.admit(now_ms, BADGE_RATE_PER_SEC) {
            return Err(());
        }
        let new = match cmd {
            BadgeCmd::Set { badge, .. } => Some(badge),
            BadgeCmd::Clear => None,
        };
        let changed = self.badge != new;
        self.badge = new;
        Ok(changed)
    }

    pub fn replay(&self, out: &mut String) {
        if let Some(b) = &self.badge {
            let _ = write!(
                out,
                "\x1b]7718;badge;set;{};{};replay\x07",
                b64_encode(b.text.as_bytes()),
                b.token.name()
            );
        }
    }
}

impl StateOwner for BadgeState {
    fn clear(&mut self) -> bool {
        self.badge.take().is_some()
    }
}

/// The `7722` marks of one pane, oldest first.
#[derive(Clone, Debug, Default)]
pub struct MarkStore {
    marks: std::collections::VecDeque<ProgramMark>,
    rate: RateWindow,
}

impl MarkStore {
    pub fn marks(&self) -> Vec<ProgramMark> {
        self.marks.iter().cloned().collect()
    }

    /// Drops marks whose line left the history (`line < base`).
    pub fn prune(&mut self, base: u64) -> bool {
        let n = self.marks.len();
        self.marks.retain(|m| m.line >= base);
        n != self.marks.len()
    }

    /// `line` is `None` on the alternate screen. `Err(())` means rate-dropped.
    fn apply(&mut self, cmd: MarkCmd, line: Option<u64>, now_ms: u64) -> Result<Option<bool>, ()> {
        let replay = matches!(cmd, MarkCmd::Add { replay: true, .. });
        if !replay && !self.rate.admit(now_ms, MARK_RATE_PER_SEC) {
            return Err(());
        }
        Ok(match cmd {
            MarkCmd::Clear => Some(self.clear()),
            MarkCmd::Add { kind, note, .. } => {
                // Row-anchored state is primary screen only: `None` is malformed.
                let Some(line) = line else {
                    return Ok(None);
                };
                let new = ProgramMark { line, kind, note };
                if let Some(cur) = self.marks.iter_mut().find(|m| m.line == line) {
                    let changed = *cur != new;
                    *cur = new;
                    Some(changed)
                } else {
                    if self.marks.len() >= MARKS_MAX {
                        self.marks.pop_front();
                    }
                    self.marks.push_back(new);
                    Some(true)
                }
            }
        })
    }
}

impl MarkStore {
    /// `StateOwner::clear` for callers that do not import the trait.
    pub fn clear_all(&mut self) -> bool {
        StateOwner::clear(self)
    }
}

impl StateOwner for MarkStore {
    fn clear(&mut self) -> bool {
        let changed = !self.marks.is_empty();
        self.marks.clear();
        changed
    }
}

/// The `7723` bindings of one pane, in registration order.
#[derive(Clone, Debug, Default)]
pub struct KeyStore {
    keys: Vec<PaneKey>,
}

impl KeyStore {
    pub fn keys(&self) -> &[PaneKey] {
        &self.keys
    }

    pub fn find(&self, chord: &str) -> Option<&PaneKey> {
        self.keys.iter().find(|k| k.chord == chord)
    }

    /// `Ok(false)` unchanged, `Err(())` refused (cap, reserved or taken chord): malformed.
    fn apply(&mut self, cmd: KeyCmd) -> Result<bool, ()> {
        match cmd {
            KeyCmd::Clear(None) => Ok(StateOwner::clear(self)),
            KeyCmd::Clear(Some(id)) => {
                let n = self.keys.len();
                self.keys.retain(|k| k.id != id);
                Ok(n != self.keys.len())
            }
            KeyCmd::Add { key, .. } => {
                if RESERVED_CHORDS.contains(&key.chord.to_ascii_lowercase().as_str())
                    || is_taken(&key.chord)
                {
                    return Err(());
                }
                // A chord belongs to one id; another id on it is a collision.
                if self
                    .keys
                    .iter()
                    .any(|k| k.chord == key.chord && k.id != key.id)
                {
                    return Err(());
                }
                if let Some(slot) = self.keys.iter_mut().find(|k| k.id == key.id) {
                    let changed = *slot != key;
                    *slot = key;
                    Ok(changed)
                } else if self.keys.len() >= KEY_BINDINGS_MAX {
                    Err(())
                } else {
                    self.keys.push(key);
                    Ok(true)
                }
            }
        }
    }

    pub fn replay(&self, out: &mut String) {
        for k in &self.keys {
            let _ = write!(
                out,
                "\x1b]7723;key;add;{};{};{};{};replay\x07",
                k.id,
                chord_syntax(&k.chord),
                k.kind.name(),
                b64_encode(k.payload.as_bytes())
            );
        }
    }
}

impl StateOwner for KeyStore {
    fn clear(&mut self) -> bool {
        let changed = !self.keys.is_empty();
        self.keys.clear();
        changed
    }
}

/// The `7724` palette entries of one pane, in registration order.
#[derive(Clone, Debug, Default)]
pub struct PaletteStore {
    entries: Vec<PaneEntry>,
}

impl PaletteStore {
    pub fn entries(&self) -> &[PaneEntry] {
        &self.entries
    }

    fn apply(&mut self, cmd: PaletteCmd) -> Result<bool, ()> {
        match cmd {
            PaletteCmd::Clear(None) => Ok(StateOwner::clear(self)),
            PaletteCmd::Clear(Some(id)) => {
                let n = self.entries.len();
                self.entries.retain(|e| e.id != id);
                Ok(n != self.entries.len())
            }
            PaletteCmd::Add { entry, .. } => {
                if let Some(slot) = self.entries.iter_mut().find(|e| e.id == entry.id) {
                    let changed = *slot != entry;
                    *slot = entry;
                    Ok(changed)
                } else if self.entries.len() >= PALETTE_ENTRIES_MAX {
                    Err(())
                } else {
                    self.entries.push(entry);
                    Ok(true)
                }
            }
        }
    }

    pub fn replay(&self, out: &mut String) {
        for e in &self.entries {
            let _ = write!(
                out,
                "\x1b]7724;palette;add;{};{};{};{};replay\x07",
                e.id,
                b64_encode(e.label.as_bytes()),
                e.kind.name(),
                b64_encode(e.payload.as_bytes())
            );
        }
    }
}

impl StateOwner for PaletteStore {
    fn clear(&mut self) -> bool {
        let changed = !self.entries.is_empty();
        self.entries.clear();
        changed
    }
}

/// Per-pane counters named in §6.4.2 item 7.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub applied: u64,
    pub malformed: u64,
    pub dropped: u64,
    pub swallowed_off: u64,
    pub swallowed_disabled: u64,
}

/// What a handled sequence asks the caller to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    Changed(Feature),
    /// The one PTY write the range may produce.
    Reply(Vec<u8>),
}

/// Everything the block keeps per pane besides the menu registry (which predates it).
#[derive(Clone, Debug, Default)]
pub struct PrivateState {
    pub counters: Counters,
    window_start: u64,
    window_count: u32,
    caps_start: u64,
    caps_count: u32,
    pub status: StatusSegments,
    pub progress: ProgressBars,
    pub tab: DeclaredTab,
    pub badge: BadgeState,
    pub marks: MarkStore,
    pub keys: KeyStore,
    pub palette: PaletteStore,
    pub folds: FoldStore,
    pub links: LinkRules,
    /// Window shared by `7723`, `7724` and `7717`.
    pane_rate: RateWindow,
    /// Stable id of the cursor row, set by the caller before each `handle`; `None` on the
    /// alternate screen.
    pub cursor_line: Option<u64>,
}

impl PrivateState {
    /// Evaluates both layers, then applies `cmd`. `st_bel` is whether the request ended in BEL.
    pub fn handle(
        &mut self,
        gate: &PrivateGate,
        menus: &mut MenuRegistry,
        cmd: PrivCmd,
        st_bel: bool,
        now_ms: u64,
    ) -> Outcome {
        // Layer 1, before anything else (including the parse result) is looked at.
        if !gate.enabled {
            self.counters.swallowed_off += 1;
            return Outcome::Nothing;
        }
        // Layer 2.
        if let Some(f) = cmd.feature() {
            if !gate.features.get(f) {
                self.counters.swallowed_disabled += 1;
                return Outcome::Nothing;
            }
        }
        if matches!(cmd, PrivCmd::Malformed(_) | PrivCmd::Reserved(_)) {
            self.counters.malformed += 1;
            return Outcome::Nothing;
        }
        if matches!(cmd, PrivCmd::CapsQuery) {
            return self.caps(gate, st_bel, now_ms);
        }
        let replay = matches!(
            cmd,
            PrivCmd::Menu(RegCmd::Add { replay: true, .. })
                | PrivCmd::Status(StatusCmd::Set { replay: true, .. })
                | PrivCmd::Progress(ProgressCmd::Set { replay: true, .. })
                | PrivCmd::Tab(TabCmd { replay: true, .. })
                | PrivCmd::Badge(BadgeCmd::Set { replay: true, .. })
                | PrivCmd::Mark(MarkCmd::Add { replay: true, .. })
                | PrivCmd::Key(KeyCmd::Add { replay: true, .. })
                | PrivCmd::Palette(PaletteCmd::Add { replay: true, .. })
                | PrivCmd::Fold(FoldCmd::Begin { replay: true, .. })
                | PrivCmd::Fold(FoldCmd::End { replay: true, .. })
                | PrivCmd::Link(LinkCmd::Add { replay: true, .. })
        );
        if !replay {
            if now_ms.saturating_sub(self.window_start) >= WINDOW_MS || self.window_count == 0 {
                self.window_start = now_ms;
                self.window_count = 0;
            }
            if self.window_count >= gate.rate.clamp(2, 100) {
                self.counters.dropped += 1;
                return Outcome::Nothing;
            }
            self.window_count += 1;
        }
        match cmd {
            PrivCmd::Menu(c) => {
                if !replay {
                    // Counts against the window `7723`/`7724` share; 7717 keeps its own limit.
                    self.pane_rate.admit(now_ms, u32::MAX);
                }
                self.counters.applied += 1;
                if menus.apply(c, now_ms) {
                    Outcome::Changed(Feature::Menu)
                } else {
                    Outcome::Nothing
                }
            }
            PrivCmd::Status(c) => match self.status.apply(c, now_ms) {
                Err(()) => {
                    self.counters.dropped += 1;
                    Outcome::Nothing
                }
                Ok(changed) => {
                    self.counters.applied += 1;
                    if changed {
                        Outcome::Changed(Feature::Status)
                    } else {
                        Outcome::Nothing
                    }
                }
            },
            PrivCmd::Progress(c) => {
                self.counters.applied += 1;
                if self.progress.apply(c, now_ms) {
                    Outcome::Changed(Feature::Progress)
                } else {
                    Outcome::Nothing
                }
            }
            PrivCmd::Tab(c) => {
                self.counters.applied += 1;
                if self.tab.apply(c) {
                    Outcome::Changed(Feature::Tab)
                } else {
                    Outcome::Nothing
                }
            }
            PrivCmd::Badge(c) => match self.badge.apply(c, now_ms) {
                Err(()) => {
                    self.counters.dropped += 1;
                    Outcome::Nothing
                }
                Ok(changed) => {
                    self.counters.applied += 1;
                    if changed {
                        Outcome::Changed(Feature::Badge)
                    } else {
                        Outcome::Nothing
                    }
                }
            },
            PrivCmd::Mark(c) => match self.marks.apply(c, self.cursor_line, now_ms) {
                Err(()) => {
                    self.counters.dropped += 1;
                    Outcome::Nothing
                }
                Ok(None) => {
                    self.counters.malformed += 1;
                    Outcome::Nothing
                }
                Ok(Some(changed)) => {
                    self.counters.applied += 1;
                    if changed {
                        Outcome::Changed(Feature::Mark)
                    } else {
                        Outcome::Nothing
                    }
                }
            },
            PrivCmd::Key(c) => {
                if !replay
                    && !self
                        .pane_rate
                        .admit(now_ms, KEY_RATE_PER_SEC.min(PALETTE_RATE_PER_SEC))
                {
                    self.counters.dropped += 1;
                    return Outcome::Nothing;
                }
                match self.keys.apply(c) {
                    Err(()) => {
                        self.counters.malformed += 1;
                        Outcome::Nothing
                    }
                    Ok(changed) => {
                        self.counters.applied += 1;
                        if changed {
                            Outcome::Changed(Feature::Key)
                        } else {
                            Outcome::Nothing
                        }
                    }
                }
            }
            PrivCmd::Palette(c) => {
                if !replay && !self.pane_rate.admit(now_ms, PALETTE_RATE_PER_SEC) {
                    self.counters.dropped += 1;
                    return Outcome::Nothing;
                }
                match self.palette.apply(c) {
                    Err(()) => {
                        self.counters.malformed += 1;
                        Outcome::Nothing
                    }
                    Ok(changed) => {
                        self.counters.applied += 1;
                        if changed {
                            Outcome::Changed(Feature::Palette)
                        } else {
                            Outcome::Nothing
                        }
                    }
                }
            }
            PrivCmd::Fold(c) => match self.folds.apply(c, self.cursor_line, now_ms) {
                Err(()) => {
                    self.counters.dropped += 1;
                    Outcome::Nothing
                }
                Ok(None) => {
                    self.counters.malformed += 1;
                    Outcome::Nothing
                }
                Ok(Some(changed)) => {
                    self.counters.applied += 1;
                    if changed {
                        Outcome::Changed(Feature::Fold)
                    } else {
                        Outcome::Nothing
                    }
                }
            },
            PrivCmd::Link(c) => match self.links.apply(c, now_ms) {
                Err(()) => {
                    self.counters.dropped += 1;
                    Outcome::Nothing
                }
                Ok(None) => {
                    self.counters.malformed += 1;
                    Outcome::Nothing
                }
                Ok(Some(changed)) => {
                    self.counters.applied += 1;
                    if changed {
                        Outcome::Changed(Feature::Link)
                    } else {
                        Outcome::Nothing
                    }
                }
            },
            _ => Outcome::Nothing,
        }
    }

    fn caps(&mut self, gate: &PrivateGate, st_bel: bool, now_ms: u64) -> Outcome {
        if !gate.replies {
            return Outcome::Nothing;
        }
        if now_ms.saturating_sub(self.caps_start) >= WINDOW_MS || self.caps_count == 0 {
            self.caps_start = now_ms;
            self.caps_count = 0;
        }
        if self.caps_count >= CAPS_REPLIES_PER_SEC {
            self.counters.dropped += 1;
            return Outcome::Nothing;
        }
        self.caps_count += 1;
        self.counters.applied += 1;
        Outcome::Reply(caps_reply(gate, st_bel))
    }

    /// Clears the state of every feature the new gate no longer allows; returns the features
    /// whose state changed. Nothing is restored when a gate goes back on.
    pub fn gate_changed(
        &mut self,
        old: &PrivateGate,
        new: &PrivateGate,
        menus: &mut MenuRegistry,
    ) -> Vec<Feature> {
        let mut out = Vec::new();
        for f in Feature::ALL {
            if old.allows(f) && !new.allows(f) && self.clear_feature(f, menus) {
                out.push(f);
            }
        }
        out
    }

    fn clear_feature(&mut self, f: Feature, menus: &mut MenuRegistry) -> bool {
        match f {
            Feature::Menu => menus.clear(),
            Feature::Status => self.status.clear(),
            Feature::Progress => self.progress.clear(),
            Feature::Tab => self.tab.clear(),
            Feature::Badge => self.badge.clear(),
            Feature::Mark => self.marks.clear(),
            Feature::Key => self.keys.clear(),
            Feature::Palette => self.palette.clear(),
            Feature::Fold => self.folds.clear(),
            Feature::Link => self.links.clear(),
            Feature::Caps => false,
        }
    }

    /// Clears all state (RIS, process exit); returns the features that had any.
    pub fn clear_all(&mut self, menus: &mut MenuRegistry) -> Vec<Feature> {
        Feature::ALL
            .into_iter()
            .filter(|f| self.clear_feature(*f, menus))
            .collect()
    }
}

/// The base64 JSON of §6.4.4 as a full reply sequence. Lists every sequence name with its
/// version, or `0` when its gate is off or it is not built yet. Never names the layer that
/// disabled a feature and carries no paths, users, pane ids or config values.
pub fn caps_reply(gate: &PrivateGate, st_bel: bool) -> Vec<u8> {
    let mut features = String::new();
    for (i, f) in Feature::ALL.iter().enumerate() {
        if i > 0 {
            features.push(',');
        }
        let v = if gate.allows(*f) { f.version() } else { 0 };
        let _ = write!(features, "\"{}\":{v}", f.name());
    }
    let json = format!(
        "{{\"v\":1,\"microterm\":\"{}\",\"features\":{{{features}}},\"limits\":{{\"status_slots\":{STATUS_SLOTS},\"status_text\":{STATUS_TEXT_MAX},\"status_tooltip\":{STATUS_TOOLTIP_MAX},\"progress_bars\":{PROGRESS_BARS},\"progress_label\":{PROGRESS_LABEL_MAX},\"badge_text\":{BADGE_TEXT_MAX},\"marks\":{MARKS_MAX},\"mark_note\":{MARK_NOTE_MAX},\"pane_keys\":{KEY_BINDINGS_MAX},\"pane_palette\":{PALETTE_ENTRIES_MAX},\"fold_regions\":{FOLD_REGIONS_MAX},\"fold_depth\":{FOLD_DEPTH_MAX},\"link_rules\":{LINK_RULES_MAX},\"max_osc\":{MAX_OSC}}},\"remote\":true}}",
        env!("CARGO_PKG_VERSION")
    );
    let mut out = format!("\x1b]7727;caps;{}", b64_encode(json.as_bytes())).into_bytes();
    out.extend_from_slice(if st_bel { b"\x07" } else { b"\x1b\\" });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyseq(id: &str, chord: &str, kind: &str, payload: &str) -> Vec<u8> {
        format!(
            "7723;key;add;{id};{chord};{kind};{}",
            b64_encode(payload.as_bytes())
        )
        .into_bytes()
    }

    fn palseq(id: &str, label: &str, kind: &str, payload: &str) -> Vec<u8> {
        format!(
            "7724;palette;add;{id};{};{kind};{}",
            b64_encode(label.as_bytes()),
            b64_encode(payload.as_bytes())
        )
        .into_bytes()
    }

    fn feed(st: &mut PrivateState, p: &[u8], now: u64) -> Outcome {
        let mut menus = MenuRegistry::default();
        st.handle(&on(), &mut menus, parse(p).unwrap(), false, now)
    }

    #[test]
    fn key_parsing_table() {
        assert!(matches!(
            parse(&keyseq("t", "ctrl+alt+t", "send", "make test")),
            Some(PrivCmd::Key(KeyCmd::Add { .. }))
        ));
        assert!(matches!(
            parse(&keyseq("t", "f5", "ctl", "pane notify")),
            Some(PrivCmd::Key(_))
        ));
        for bad in [
            keyseq("t", "t", "send", "x"),
            keyseq("t", "shift+t", "send", "x"),
            keyseq("t", "ctrl+nope", "send", "x"),
            keyseq("t", "ctrl+t", "spawn", "[]"),
            keyseq("T", "ctrl+t", "send", "x"),
            keyseq("t", "ctrl+t", "send", ""),
            keyseq("t", "ctrl+ctrl+t", "send", "x"),
        ] {
            assert!(matches!(
                parse(&bad),
                Some(PrivCmd::Malformed(Some(Feature::Key)))
            ));
        }
        assert_eq!(
            canonical_chord("cmd+shift+P").as_deref(),
            Some("shift-cmd-p")
        );
        assert_eq!(canonical_chord("ctrl+alt+t").as_deref(), Some("ctrl-alt-t"));
    }

    #[test]
    fn palette_parsing_strips_controls_and_rejects_spawn() {
        let Some(PrivCmd::Palette(PaletteCmd::Add { entry, .. })) =
            parse(&palseq("r", "Run\x1b[31m tests\n", "send", "make test"))
        else {
            panic!()
        };
        assert_eq!(entry.label, "Run[31m tests");
        assert!(matches!(
            parse(&palseq("r", "x", "spawn", "[]")),
            Some(PrivCmd::Malformed(Some(Feature::Palette)))
        ));
        assert!(matches!(
            parse(&palseq("r", &"a".repeat(257), "send", "x")),
            Some(PrivCmd::Malformed(_))
        ));
        assert!(matches!(
            parse(&palseq("r", "ok", "send", &"a".repeat(1025))),
            Some(PrivCmd::Malformed(_))
        ));
    }

    #[test]
    fn reserved_chords_are_malformed_and_counted() {
        let mut st = PrivateState::default();
        for (i, c) in ["cmd+q", "cmd+w", "cmd+c", "cmd+v", "cmd+shift+p"]
            .iter()
            .enumerate()
        {
            let out = feed(&mut st, &keyseq("k", c, "send", "x"), i as u64 * 300);
            assert_eq!(out, Outcome::Nothing);
        }
        assert_eq!(st.counters.malformed, 5);
        assert!(st.keys.keys().is_empty());
    }

    #[test]
    fn keys_cap_replace_clear_and_rate() {
        let mut st = PrivateState::default();
        assert_eq!(
            feed(&mut st, &keyseq("a", "ctrl+alt+a", "send", "x"), 0),
            Outcome::Changed(Feature::Key)
        );
        // Same id replaces, same chord on another id collides.
        assert_eq!(
            feed(&mut st, &keyseq("a", "ctrl+alt+a", "send", "y"), 1),
            Outcome::Changed(Feature::Key)
        );
        feed(&mut st, &keyseq("b", "ctrl+alt+a", "send", "x"), 2);
        assert_eq!(st.counters.malformed, 1);
        assert_eq!(st.keys.keys().len(), 1);
        // 5 per second, shared window.
        for i in 0..6 {
            feed(
                &mut st,
                &keyseq(
                    "c",
                    &format!("ctrl+alt+{}", (b'b' + i) as char),
                    "send",
                    "x",
                ),
                3,
            );
        }
        assert!(st.counters.dropped >= 1);
        assert_eq!(
            feed(&mut st, b"7723;key;clear", 5000),
            Outcome::Changed(Feature::Key)
        );
        assert!(st.keys.keys().is_empty());
    }

    #[test]
    fn key_cap_is_sixteen() {
        let mut st = PrivateState::default();
        for i in 0..17u64 {
            let c = format!("ctrl+alt+{}", (b'a' + i as u8) as char);
            feed(
                &mut st,
                &keyseq(&format!("k{i}"), &c, "send", "x"),
                i * 1000,
            );
        }
        assert_eq!(st.keys.keys().len(), KEY_BINDINGS_MAX);
        assert_eq!(st.counters.malformed, 1);
    }

    #[test]
    fn palette_shares_the_window_with_menu_and_keys() {
        let mut st = PrivateState::default();
        let mut menus = MenuRegistry::default();
        let gate = on();
        for i in 0..8 {
            let seq = format!(
                "7717;menu;add;m{i};{};send;{}",
                b64_encode(b"x"),
                b64_encode(b"x")
            );
            st.handle(&gate, &mut menus, parse(seq.as_bytes()).unwrap(), false, 0);
        }
        let mut dropped_before = st.counters.dropped;
        for i in 0..4 {
            st.handle(
                &gate,
                &mut menus,
                parse(&palseq(&format!("p{i}"), "l", "send", "x")).unwrap(),
                false,
                0,
            );
        }
        // 8 menu adds + 2 palette adds fill the window of 10.
        assert_eq!(st.palette.entries().len(), 2);
        dropped_before += 2;
        assert_eq!(st.counters.dropped, dropped_before);
    }

    #[test]
    fn replay_round_trips_keys_and_palette() {
        let mut st = PrivateState::default();
        feed(&mut st, &keyseq("t", "ctrl+alt+t", "send", "make test"), 0);
        feed(&mut st, &palseq("r", "Run tests", "ctl", "pane notify"), 0);
        let mut out = String::new();
        st.keys.replay(&mut out);
        st.palette.replay(&mut out);
        let mut st2 = PrivateState::default();
        for seq in out.split('\x07').filter(|s| !s.is_empty()) {
            let p = seq.trim_start_matches('\x1b').trim_start_matches(']');
            feed(&mut st2, p.as_bytes(), 0);
        }
        assert_eq!(st2.keys.keys(), st.keys.keys());
        assert_eq!(st2.palette.entries(), st.palette.entries());
    }

    #[test]
    fn gates_and_clear_all_remove_keys_and_palette() {
        let mut st = PrivateState::default();
        let mut menus = MenuRegistry::default();
        feed(&mut st, &keyseq("t", "ctrl+alt+t", "send", "x"), 0);
        feed(&mut st, &palseq("r", "R", "send", "x"), 0);
        let mut off = on();
        off.features.set(Feature::Key, false);
        assert_eq!(st.gate_changed(&on(), &off, &mut menus), vec![Feature::Key]);
        assert!(st.keys.keys().is_empty());
        assert_eq!(st.palette.entries().len(), 1);
        assert_eq!(st.clear_all(&mut menus), vec![Feature::Palette]);
    }

    #[test]
    fn gpui_chords_and_dash_key_round_trip() {
        assert_eq!(chord_syntax("ctrl-alt-t"), "ctrl+alt+t");
        assert_eq!(chord_syntax("ctrl-alt--"), "ctrl+alt+-");
        assert_eq!(
            canonical_chord(&chord_syntax("ctrl-alt--")).as_deref(),
            Some("ctrl-alt--")
        );
        assert_eq!(chord_syntax("f5"), "f5");
    }

    #[test]
    fn taken_chords_are_refused() {
        set_taken_chords(["ctrl+alt+z".to_string()]);
        let mut st = PrivateState::default();
        feed(&mut st, &keyseq("z", "ctrl+alt+z", "send", "x"), 0);
        set_taken_chords(Vec::<String>::new());
        assert!(st.keys.keys().is_empty());
        assert_eq!(st.counters.malformed, 1);
    }

    fn on() -> PrivateGate {
        PrivateGate {
            enabled: true,
            ..Default::default()
        }
    }

    fn set(slot: &str, text: &str, level: &str) -> Vec<u8> {
        format!(
            "7725;status;set;{slot};{};{level}",
            b64_encode(text.as_bytes())
        )
        .into_bytes()
    }

    fn run(
        st: &mut PrivateState,
        menus: &mut MenuRegistry,
        gate: &PrivateGate,
        p: &[u8],
        t: u64,
    ) -> Outcome {
        st.handle(gate, menus, parse(p).unwrap(), true, t)
    }

    fn bar(id: &str, pct: &str) -> Vec<u8> {
        format!("7720;progress;set;{id};{pct};{}", b64_encode(b"Build")).into_bytes()
    }

    fn pr(
        st: &mut PrivateState,
        menus: &mut MenuRegistry,
        gate: &PrivateGate,
        p: &[u8],
        t: u64,
    ) -> Outcome {
        run(st, menus, gate, p, t)
    }

    #[test]
    fn progress_parsing_tables() {
        assert!(matches!(
            parse(&bar("a", "40")),
            Some(PrivCmd::Progress(ProgressCmd::Set { replay: false, .. }))
        ));
        let with_eta = format!("7720;progress;set;a;-;{};80", b64_encode(b"x"));
        let Some(PrivCmd::Progress(ProgressCmd::Set { bar, .. })) = parse(with_eta.as_bytes())
        else {
            panic!()
        };
        assert_eq!((bar.pct, bar.eta_s), (None, Some(80)));
        for bad in [
            "7720;progress;set;a;101;eA==".to_string(),
            "7720;progress;set;a;x;eA==".to_string(),
            "7720;progress;set;A;1;eA==".to_string(),
            "7720;progress;set;a;1;!!".to_string(),
            "7720;progress;set;a;1;".to_string(),
            format!(
                "7720;progress;set;a;1;{}",
                b64_encode("é".repeat(49).as_bytes())
            ),
            "7720;progress;set;a;1;eA==;360000".to_string(),
            "7720;progress;set;a;1;eA==;-1".to_string(),
            "7720;progress;set;a;1;eA==;5;6".to_string(),
            "7720;progress;set;a;1;eA==;;replay".to_string() + "x",
            "7720;progress;done".to_string(),
            "7720;progress;nope;a".to_string(),
            "7720;tab;done;a".to_string(),
        ] {
            assert_eq!(
                parse(bad.as_bytes()),
                Some(PrivCmd::Malformed(Some(Feature::Progress))),
                "{bad}"
            );
        }
        assert!(matches!(
            parse(b"7720;progress;set;a;100;eA==;359999"),
            Some(PrivCmd::Progress(_))
        ));
        assert!(matches!(
            parse(b"7720;progress;set;a;5;eA==;;replay"),
            Some(PrivCmd::Progress(ProgressCmd::Set { replay: true, .. }))
        ));
        assert!(matches!(
            parse(b"7720;progress;clear"),
            Some(PrivCmd::Progress(_))
        ));
        assert!(matches!(
            parse(b"7720;progress;clear;a"),
            Some(PrivCmd::Progress(_))
        ));
    }

    #[test]
    fn bar_limits_done_and_staleness() {
        let (mut st, mut m, g) = (PrivateState::default(), MenuRegistry::default(), on());
        for (i, id) in ["a", "b", "c", "d", "e", "f", "g"].iter().enumerate() {
            pr(&mut st, &mut m, &g, &bar(id, "1"), i as u64 * 200);
        }
        assert_eq!(st.progress.bars().len(), PROGRESS_BARS);
        pr(&mut st, &mut m, &g, b"7720;progress;done;a", 2000);
        let a = st
            .progress
            .bars()
            .into_iter()
            .find(|b| b.id == "a")
            .unwrap();
        assert_eq!((a.pct, a.done), (Some(100), true));
        assert!(!st.progress.expire(3999));
        assert!(st.progress.expire(4000));
        assert_eq!(st.progress.bars().len(), 5);
        // Ten minutes without an update.
        assert!(!st.progress.expire(199 + PROGRESS_STALE_MS));
        assert!(st.progress.expire(200 + PROGRESS_STALE_MS));
        assert_eq!(st.progress.bars().len(), 4);
        assert!(st.progress.expire(1000 + PROGRESS_STALE_MS));
        assert!(st.progress.bars().is_empty());
    }

    #[test]
    fn progress_set_rate_coalesces_newest_wins() {
        let (mut st, mut m) = (PrivateState::default(), MenuRegistry::default());
        let g = PrivateGate {
            enabled: true,
            rate: 100,
            ..Default::default()
        };
        for i in 0..30u32 {
            pr(
                &mut st,
                &mut m,
                &g,
                &bar("a", &(i.min(99)).to_string()),
                10_000 + i as u64,
            );
        }
        // Ten applied in the window, the rest pending: the last value shows after it rolls.
        assert_eq!(st.progress.bars()[0].pct, Some(9));
        assert_eq!(st.counters.dropped, 0);
        assert!(st.progress.expire(11_100));
        assert_eq!(st.progress.bars()[0].pct, Some(29));
    }

    #[test]
    fn tab_color_syntax_clear_and_replay() {
        let (mut st, mut m, g) = (PrivateState::default(), MenuRegistry::default(), on());
        for ok in ["orange", "#f59e42", "prod-red", ""] {
            let p = format!("7719;tab;color;{ok}");
            assert!(matches!(parse(p.as_bytes()), Some(PrivCmd::Tab(_))), "{ok}");
        }
        for bad in ["#12345", "#gggggg", "Red", "a b", "#1234567", "x;y"] {
            let p = format!("7719;tab;color;{bad}");
            assert_eq!(
                parse(p.as_bytes()),
                Some(PrivCmd::Malformed(Some(Feature::Tab))),
                "{bad}"
            );
        }
        assert_eq!(
            parse(b"7719;tab;color"),
            Some(PrivCmd::Malformed(Some(Feature::Tab)))
        );
        assert_eq!(
            pr(&mut st, &mut m, &g, b"7719;tab;color;orange", 0),
            Outcome::Changed(Feature::Tab)
        );
        assert_eq!(st.tab.color(), Some("orange"));
        let mut out = String::new();
        st.tab.replay(&mut out);
        assert_eq!(out, "\x1b]7719;tab;color;orange;replay\x07");
        assert_eq!(
            pr(&mut st, &mut m, &g, b"7719;tab;color;orange", 1),
            Outcome::Nothing
        );
        assert_eq!(
            pr(&mut st, &mut m, &g, b"7719;tab;color;", 2),
            Outcome::Changed(Feature::Tab)
        );
        assert_eq!(st.tab.color(), None);
    }

    #[test]
    fn progress_and_tab_follow_the_gates_and_never_write() {
        let (mut st, mut m, g) = (PrivateState::default(), MenuRegistry::default(), on());
        pr(&mut st, &mut m, &g, &bar("a", "5"), 0);
        pr(&mut st, &mut m, &g, b"7719;tab;color;red", 0);
        let off = PrivateGate::default();
        let cleared = st.gate_changed(&g, &off, &mut m);
        assert!(cleared.contains(&Feature::Progress) && cleared.contains(&Feature::Tab));
        assert!(st.progress.bars().is_empty() && st.tab.color().is_none());
        assert_eq!(
            pr(&mut st, &mut m, &off, &bar("a", "5"), 5000),
            Outcome::Nothing
        );
        assert!(st.progress.bars().is_empty());
        assert_eq!(st.counters.swallowed_off, 1);
        // One feature gate.
        let mut g2 = on();
        g2.features.set(Feature::Tab, false);
        assert_eq!(
            pr(&mut st, &mut m, &g2, b"7719;tab;color;red", 9000),
            Outcome::Nothing
        );
        assert_eq!(st.counters.swallowed_disabled, 1);
        assert!(matches!(
            pr(&mut st, &mut m, &g2, &bar("a", "5"), 9000),
            Outcome::Changed(Feature::Progress)
        ));
        // RIS / exit.
        assert_eq!(st.clear_all(&mut m), vec![Feature::Progress]);
    }

    #[test]
    fn only_the_block_is_private() {
        assert!(parse(b"7;file:///x").is_none());
        assert!(parse(b"777;notify;a;b").is_none());
        assert!(parse(b"133;A").is_none());
        assert!(parse(b"7800;x").is_none());
        assert!(parse(b"7725;status;clear").is_some());
    }

    #[test]
    fn namespace_must_match_number() {
        assert_eq!(
            parse(b"7725;caps;query"),
            Some(PrivCmd::Malformed(Some(Feature::Status)))
        );
        assert_eq!(parse(b"7727;caps;query"), Some(PrivCmd::CapsQuery));
        assert_eq!(parse(b"7799;x;y"), Some(PrivCmd::Reserved(None)));
        assert_eq!(
            parse(b"7721;fold;x"),
            Some(PrivCmd::Malformed(Some(Feature::Fold)))
        );
        assert_eq!(
            parse(b"7718;mark;clear"),
            Some(PrivCmd::Malformed(Some(Feature::Badge)))
        );
    }

    fn badge(text: &str, tok: &str) -> Vec<u8> {
        format!("7718;badge;set;{};{tok}", b64_encode(text.as_bytes())).into_bytes()
    }

    fn mark(kind: &str, note: &str) -> Vec<u8> {
        format!("7722;mark;add;{kind};{}", b64_encode(note.as_bytes())).into_bytes()
    }

    #[test]
    fn badge_parsing_tables() {
        let Some(PrivCmd::Badge(BadgeCmd::Set { badge: b, replay })) =
            parse(&badge("PROD", "error"))
        else {
            panic!()
        };
        assert_eq!(
            (b.text.as_str(), b.token, replay),
            ("PROD", BadgeToken::Error, false)
        );
        let Some(PrivCmd::Badge(BadgeCmd::Set { badge: b, .. })) =
            parse(format!("7718;badge;set;{}", b64_encode(b"dev")).as_bytes())
        else {
            panic!()
        };
        assert_eq!(b.token, BadgeToken::Muted);
        assert_eq!(
            parse(b"7718;badge;clear"),
            Some(PrivCmd::Badge(BadgeCmd::Clear))
        );
        for bad in [
            String::from_utf8(badge("PROD", "red")).unwrap(),
            String::from_utf8(badge("PROD", "#ff0000")).unwrap(),
            String::from_utf8(badge(&"x".repeat(25), "error")).unwrap(),
            "7718;badge;set;!!".to_string(),
            "7718;badge;set;".to_string(),
            "7718;badge;set".to_string(),
            "7718;badge;clear;x".to_string(),
            "7718;badge;nope".to_string(),
            format!("7718;badge;set;{};error;x", b64_encode(b"a")),
        ] {
            assert_eq!(
                parse(bad.as_bytes()),
                Some(PrivCmd::Malformed(Some(Feature::Badge))),
                "{bad}"
            );
        }
        // Control characters and line breaks are removed: one line only.
        let Some(PrivCmd::Badge(BadgeCmd::Set { badge: b, .. })) =
            parse(&badge("A\nB\x1b", "accent"))
        else {
            panic!()
        };
        assert_eq!(b.text, "A B");
        assert!(parse(&badge(&"é".repeat(24), "muted")).is_some());
    }

    #[test]
    fn mark_parsing_tables() {
        let Some(PrivCmd::Mark(MarkCmd::Add { kind, note, replay })) = parse(&mark("warn", "slow"))
        else {
            panic!()
        };
        assert_eq!(
            (kind, note.as_str(), replay),
            (MarkKind::Warn, "slow", false)
        );
        assert_eq!(
            parse(b"7722;mark;clear"),
            Some(PrivCmd::Mark(MarkCmd::Clear))
        );
        assert!(parse(&mark("info", &"x".repeat(120))).is_some());
        for bad in [
            String::from_utf8(mark("fatal", "x")).unwrap(),
            String::from_utf8(mark("info", &"x".repeat(121))).unwrap(),
            "7722;mark;add;info".to_string(),
            "7722;mark;add;info;!!".to_string(),
            "7722;mark;clear;x".to_string(),
            "7722;mark;del".to_string(),
            "7722;badge;clear".to_string(),
        ] {
            assert_eq!(
                parse(bad.as_bytes()),
                Some(PrivCmd::Malformed(Some(Feature::Mark))),
                "{bad}"
            );
        }
    }

    #[test]
    fn badge_follows_gates_rate_and_replay() {
        let mut st = PrivateState::default();
        let mut menus = MenuRegistry::default();
        let off = PrivateGate::default();
        assert_eq!(
            run(&mut st, &mut menus, &off, &badge("PROD", "error"), 0),
            Outcome::Nothing
        );
        assert!(st.badge.badge().is_none());
        let g = on();
        assert_eq!(
            run(&mut st, &mut menus, &g, &badge("PROD", "error"), 0),
            Outcome::Changed(Feature::Badge)
        );
        assert_eq!(st.badge.badge().unwrap().text, "PROD");
        let mut out = String::new();
        st.badge.replay(&mut out);
        assert!(out.starts_with("\x1b]7718;badge;set;") && out.ends_with(";error;replay\x07"));
        // 2 per second.
        run(&mut st, &mut menus, &g, &badge("A", "muted"), 10);
        let d = st.counters.dropped;
        assert_eq!(
            run(&mut st, &mut menus, &g, &badge("B", "muted"), 20),
            Outcome::Nothing
        );
        assert_eq!(st.counters.dropped, d + 1);
        assert_eq!(st.badge.badge().unwrap().text, "A");
        // Replay is exempt from the windows.
        let r = format!("7718;badge;set;{};accent;replay", b64_encode(b"B"));
        assert_eq!(
            run(&mut st, &mut menus, &g, r.as_bytes(), 30),
            Outcome::Changed(Feature::Badge)
        );
        // A feature gate clears and swallows.
        let mut no = g;
        no.features.set(Feature::Badge, false);
        assert_eq!(st.gate_changed(&g, &no, &mut menus), vec![Feature::Badge]);
        assert!(st.badge.badge().is_none());
        assert_eq!(
            run(&mut st, &mut menus, &no, &badge("X", "muted"), 5000),
            Outcome::Nothing
        );
        assert_eq!(st.counters.swallowed_disabled, 1);
    }

    #[test]
    fn marks_anchor_dedupe_evict_and_prune() {
        let mut st = PrivateState::default();
        let mut menus = MenuRegistry::default();
        let g = PrivateGate { rate: 100, ..on() };
        st.cursor_line = Some(7);
        assert_eq!(
            run(&mut st, &mut menus, &g, &mark("error", "a"), 0),
            Outcome::Changed(Feature::Mark)
        );
        // Same row: replaced.
        run(&mut st, &mut menus, &g, &mark("warn", "b"), 1);
        let m = st.marks.marks();
        assert_eq!(m.len(), 1);
        assert_eq!(
            (m[0].line, m[0].kind, m[0].note.as_str()),
            (7, MarkKind::Warn, "b")
        );
        // Alternate screen: malformed.
        st.cursor_line = None;
        assert_eq!(
            run(&mut st, &mut menus, &g, &mark("info", "c"), 2),
            Outcome::Nothing
        );
        assert_eq!(st.counters.malformed, 1);
        assert_eq!(st.marks.marks().len(), 1);
        // 500 cap, oldest evicted. Replay marks bypass the 20/s window.
        for i in 0..520u64 {
            st.cursor_line = Some(1000 + i);
            let r = format!("7722;mark;add;info;{};replay", b64_encode(b"n"));
            run(&mut st, &mut menus, &g, r.as_bytes(), 3);
        }
        let m = st.marks.marks();
        assert_eq!(m.len(), MARKS_MAX);
        assert_eq!(m[0].line, 1020);
        assert!(st.marks.prune(1500));
        assert_eq!(st.marks.marks()[0].line, 1500);
        assert_eq!(
            run(&mut st, &mut menus, &g, b"7722;mark;clear", 4),
            Outcome::Changed(Feature::Mark)
        );
        assert!(st.marks.marks().is_empty());
    }

    #[test]
    fn marks_are_rate_limited_to_twenty_per_second() {
        let mut st = PrivateState::default();
        let mut menus = MenuRegistry::default();
        let g = PrivateGate { rate: 100, ..on() };
        for i in 0..30u64 {
            st.cursor_line = Some(i);
            run(&mut st, &mut menus, &g, &mark("info", "x"), 5);
        }
        assert_eq!(st.marks.marks().len(), 20);
        assert_eq!(st.counters.dropped, 10);
    }

    #[test]
    fn status_parsing_tables() {
        let ok = parse(&set("git", "main ✔", "ok")).unwrap();
        let PrivCmd::Status(StatusCmd::Set { seg, replay }) = ok else {
            panic!()
        };
        assert_eq!(
            (seg.slot.as_str(), seg.text.as_str(), seg.level),
            ("git", "main ✔", Level::Ok)
        );
        assert!(!replay);
        let bad: Vec<Vec<u8>> = vec![
            b"7725;status;set;git".to_vec(),
            b"7725;status;set;Git;eA==".to_vec(),
            b"7725;status;set;git;***".to_vec(),
            b"7725;status;set;git;/w==".to_vec(), // invalid UTF-8
            b"7725;status;set;git;eA==;loud".to_vec(),
            b"7725;status;set;git;eA==;ok;eA==;nope".to_vec(),
            b"7725;status;set;git;;ok".to_vec(),
            b"7725;status;frob".to_vec(),
            b"7725;status;clear;a;b".to_vec(),
            set("git", &"x".repeat(STATUS_TEXT_MAX + 1), "ok"),
            format!(
                "7725;status;set;git;eA==;ok;{}",
                b64_encode("t".repeat(STATUS_TOOLTIP_MAX + 1).as_bytes())
            )
            .into_bytes(),
        ];
        for b in bad {
            assert_eq!(
                parse(&b),
                Some(PrivCmd::Malformed(Some(Feature::Status))),
                "{}",
                String::from_utf8_lossy(&b)
            );
        }
    }

    #[test]
    fn oversized_string_is_malformed() {
        let mut p = b"7725;status;set;git;".to_vec();
        p.resize(MAX_OSC + 1, b'A');
        assert_eq!(parse(&p), Some(PrivCmd::Malformed(Some(Feature::Status))));
    }

    #[test]
    fn sanitiser_strips_controls_and_bidi() {
        assert_eq!(sanitize("a\x1b[31m\u{202e}b\n\t c\u{85}"), "a[31mb c");
    }

    #[test]
    fn master_switch_off_swallows_everything() {
        let (mut st, mut m) = (PrivateState::default(), MenuRegistry::default());
        let gate = PrivateGate::default();
        for n in 7700..=7799u16 {
            let p = format!("{n};x;y;z");
            if let Some(c) = parse(p.as_bytes()) {
                assert_eq!(st.handle(&gate, &mut m, c, true, 0), Outcome::Nothing);
            }
        }
        for p in [
            &b"7727;caps;query"[..],
            b"7717;menu;add;a;eA==;send;eA==",
            &set("a", "x", "ok"),
        ] {
            assert_eq!(run(&mut st, &mut m, &gate, p, 0), Outcome::Nothing);
        }
        assert_eq!(st.counters.swallowed_off, 100 + 3);
        assert_eq!(
            st.counters.applied + st.counters.malformed + st.counters.dropped,
            0
        );
        assert!(st.status.segments().is_empty() && m.items().is_empty());
    }

    #[test]
    fn feature_gate_swallows_and_counts() {
        let (mut st, mut m) = (PrivateState::default(), MenuRegistry::default());
        let mut gate = on();
        gate.features.set(Feature::Status, false);
        assert_eq!(
            run(&mut st, &mut m, &gate, &set("a", "x", "ok"), 0),
            Outcome::Nothing
        );
        assert_eq!(st.counters.swallowed_disabled, 1);
        assert!(st.status.segments().is_empty());
        // Caps reports it as 0 and the others as built.
        let r = String::from_utf8(caps_reply(&gate, true)).unwrap();
        let json = String::from_utf8(
            b64_decode(
                r.strip_prefix("\x1b]7727;caps;")
                    .unwrap()
                    .strip_suffix('\x07')
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(
            json.contains("\"status\":0") && json.contains("\"caps\":1"),
            "{json}"
        );
        assert!(
            json.contains("\"fold\":1")
                && json.contains("\"badge\":1")
                && json.contains("\"progress\":1")
        );
    }

    #[test]
    fn caps_reply_copies_terminator_and_is_rate_limited() {
        let (mut st, mut m) = (PrivateState::default(), MenuRegistry::default());
        let gate = on();
        let q = parse(b"7727;caps;query").unwrap();
        let Outcome::Reply(r) = st.handle(&gate, &mut m, q.clone(), false, 0) else {
            panic!()
        };
        assert!(r.ends_with(b"\x1b\\") && r.starts_with(b"\x1b]7727;caps;"));
        assert!(matches!(
            st.handle(&gate, &mut m, q.clone(), true, 10),
            Outcome::Reply(_)
        ));
        assert_eq!(
            st.handle(&gate, &mut m, q.clone(), true, 20),
            Outcome::Nothing
        );
        assert!(matches!(
            st.handle(&gate, &mut m, q, true, 1100),
            Outcome::Reply(_)
        ));
    }

    #[test]
    fn caps_reply_never_leaks_config() {
        let json_src = String::from_utf8(caps_reply(&on(), true)).unwrap();
        for bad in ["/", "pane", "user", "enabled"] {
            assert!(!json_src.contains(bad), "{bad}");
        }
    }

    #[test]
    fn no_other_sequence_replies() {
        let (mut st, mut m) = (PrivateState::default(), MenuRegistry::default());
        let gate = on();
        for n in 7700..=7799u16 {
            for ns in ["menu", "caps", "status", "x"] {
                for tail in [
                    "",
                    ";query",
                    ";set;a;eA==",
                    ";clear",
                    ";add;a;eA==;send;eA==",
                ] {
                    let p = format!("{n};{ns}{tail}");
                    if let Some(c) = parse(p.as_bytes()) {
                        let o = st.handle(&gate, &mut m, c, true, n as u64 * 5000);
                        let is_query = n == 7727 && ns == "caps" && tail == ";query";
                        assert_eq!(matches!(o, Outcome::Reply(_)), is_query, "{p}");
                    }
                }
            }
        }
    }

    #[test]
    fn global_rate_window_drops_and_replay_is_exempt() {
        let (mut st, mut m) = (PrivateState::default(), MenuRegistry::default());
        let mut gate = on();
        gate.rate = 2;
        let slots = ["a", "b", "c"];
        for (i, s) in slots.iter().enumerate() {
            run(&mut st, &mut m, &gate, &set(s, "x", "ok"), i as u64);
        }
        assert_eq!(st.counters.dropped, 1);
        let rp = format!("7725;status;set;d;{};ok;;replay", b64_encode(b"x"));
        run(&mut st, &mut m, &gate, rp.as_bytes(), 5);
        assert_eq!(st.counters.dropped, 1);
        assert_eq!(st.status.segments().len(), 3);
        run(&mut st, &mut m, &gate, &set("c", "x", "ok"), 1500);
        assert_eq!(st.status.segments().len(), 4);
    }

    #[test]
    fn status_limits_and_order() {
        let (mut st, mut m) = (PrivateState::default(), MenuRegistry::default());
        let gate = PrivateGate { rate: 100, ..on() };
        // 5 per second for 7725.
        for i in 0..8u64 {
            run(
                &mut st,
                &mut m,
                &gate,
                &set(&format!("s{}", 7 - i), "x", "ok"),
                i * 1000,
            );
        }
        let slots: Vec<_> = st.status.segments().into_iter().map(|s| s.slot).collect();
        assert_eq!(slots.len(), 8);
        assert!(slots.windows(2).all(|w| w[0] < w[1]), "slot-name order");
        // A ninth slot is refused; replacing one is fine.
        run(&mut st, &mut m, &gate, &set("zz", "x", "ok"), 20_000);
        assert_eq!(st.status.segments().len(), 8);
        assert_eq!(
            run(&mut st, &mut m, &gate, &set("s0", "y", "warn"), 20_001),
            Outcome::Changed(Feature::Status)
        );
        let burst = PrivateGate { rate: 100, ..on() };
        for i in 0..7 {
            run(
                &mut st,
                &mut m,
                &burst,
                &set("s0", &format!("n{i}"), "ok"),
                30_000 + i,
            );
        }
        assert!(st.counters.dropped >= 2);
    }

    #[test]
    fn gate_off_clears_and_nothing_comes_back() {
        let (mut st, mut m) = (PrivateState::default(), MenuRegistry::default());
        let gate = on();
        run(&mut st, &mut m, &gate, &set("a", "x", "ok"), 0);
        run(&mut st, &mut m, &gate, b"7717;menu;add;a;eA==;send;eA==", 0);
        let off = PrivateGate::default();
        let changed = st.gate_changed(&gate, &off, &mut m);
        assert!(changed.contains(&Feature::Status) && changed.contains(&Feature::Menu));
        assert!(st.status.segments().is_empty() && m.items().is_empty());
        assert!(st.gate_changed(&off, &gate, &mut m).is_empty());
        assert!(st.status.segments().is_empty() && m.items().is_empty());
        // One feature going off clears only that one.
        run(&mut st, &mut m, &gate, &set("a", "x", "ok"), 5000);
        let mut g2 = gate;
        g2.features.set(Feature::Status, false);
        assert_eq!(st.gate_changed(&gate, &g2, &mut m), vec![Feature::Status]);
    }

    #[test]
    fn status_replay_round_trips() {
        let (mut st, mut m) = (PrivateState::default(), MenuRegistry::default());
        let gate = on();
        let p = format!(
            "7725;status;set;git;{};warn;{}",
            b64_encode("a;b".as_bytes()),
            b64_encode(b"tip")
        );
        run(&mut st, &mut m, &gate, p.as_bytes(), 0);
        let mut s = String::new();
        st.status.replay(&mut s);
        let body = s
            .strip_prefix("\x1b]")
            .unwrap()
            .strip_suffix('\x07')
            .unwrap();
        let (mut st2, mut m2) = (PrivateState::default(), MenuRegistry::default());
        run(&mut st2, &mut m2, &gate, body.as_bytes(), 0);
        assert_eq!(st2.status.segments(), st.status.segments());
    }

    #[test]
    fn no_input_can_turn_the_switch_on() {
        // The gate is a value the handler only reads: handling arbitrary input cannot alter it.
        let gate = PrivateGate::default();
        let (mut st, mut m) = (PrivateState::default(), MenuRegistry::default());
        let mut seed = 0x1234_5678u64;
        for _ in 0..5000 {
            let mut p = format!("77{:02};", seed % 100).into_bytes();
            for _ in 0..(seed % 40) {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                p.push(b" ;abcdefghijklmnopqrstuvwxyz0123456789=/+"[(seed >> 33) as usize % 41]);
            }
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            if let Some(c) = parse(&p) {
                assert_eq!(st.handle(&gate, &mut m, c, true, 0), Outcome::Nothing);
            }
        }
        assert!(!gate.enabled);
        assert_eq!(st.counters.applied, 0);
    }

    fn b(s: &str) -> String {
        b64_encode(s.as_bytes())
    }

    fn fold_begin(id: &str, title: &str) -> Vec<u8> {
        format!("7721;fold;begin;{id};{}", b(title)).into_bytes()
    }

    fn at(st: &mut PrivateState, line: u64, p: &[u8], now: u64) -> Outcome {
        st.cursor_line = Some(line);
        feed(st, p, now)
    }

    #[test]
    fn fold_nesting_end_and_malformed() {
        let mut st = PrivateState::default();
        assert_eq!(
            at(&mut st, 3, &fold_begin("a", "Build"), 0),
            Outcome::Changed(Feature::Fold)
        );
        at(&mut st, 4, &fold_begin("b", "Inner"), 0);
        // Duplicate open id, and ending the outer one first (overlap) are malformed.
        at(&mut st, 4, &fold_begin("b", "Again"), 0);
        at(&mut st, 5, b"7721;fold;end;a", 0);
        assert_eq!(st.counters.malformed, 2);
        at(&mut st, 6, b"7721;fold;end;b", 0);
        at(&mut st, 7, b"7721;fold;end;a", 0);
        let r = st.folds.regions();
        assert_eq!((r[0].start, r[0].end, r[0].depth), (3, Some(7), 0));
        assert_eq!((r[1].start, r[1].end, r[1].depth), (4, Some(6), 1));
        // Unmatched end.
        at(&mut st, 8, b"7721;fold;end;zz", 0);
        assert_eq!(st.counters.malformed, 3);
    }

    #[test]
    fn fold_depth_cap_title_and_alt_screen() {
        let mut st = PrivateState::default();
        for i in 0..5u64 {
            at(&mut st, i, &fold_begin(&format!("d{i}"), "t"), 0);
        }
        assert_eq!(st.folds.regions().len(), FOLD_DEPTH_MAX);
        assert_eq!(st.counters.malformed, 1);
        assert!(parse(&fold_begin("x", &"é".repeat(81)))
            .is_some_and(|c| matches!(c, PrivCmd::Malformed(_))));
        // Alternate screen: no cursor line.
        st.cursor_line = None;
        feed(&mut st, &fold_begin("alt", "t"), 5000);
        assert_eq!(st.counters.malformed, 2);
    }

    #[test]
    fn fold_cap_rate_prune_and_clear() {
        let mut st = PrivateState::default();
        for i in 0..205u64 {
            let now = i / FOLD_RATE_PER_SEC as u64 * 1000;
            at(&mut st, i, &fold_begin("a", "t"), now);
            at(&mut st, i + 1, b"7721;fold;end;a", now);
        }
        assert!(st.folds.regions().len() <= FOLD_REGIONS_MAX);
        assert!(st.counters.dropped > 0 || st.folds.regions().len() < 205);
        assert!(st.folds.prune(10_000));
        assert!(st.folds.regions().is_empty());
        at(&mut st, 1, &fold_begin("a", "t"), 1_000_000);
        assert!(st.folds.clear());
    }

    fn link_add(id: &str, re: &str, tpl: &str) -> Vec<u8> {
        format!("7726;link;add;{id};{};{}", b(re), b(tpl)).into_bytes()
    }

    #[test]
    fn link_parsing_cap_and_rejected_regexes() {
        let ok = link_add("issue", r"#(\d+)", "https://example.com/issues/$1");
        assert!(matches!(
            parse(&ok),
            Some(PrivCmd::Link(LinkCmd::Add { .. }))
        ));
        for bad in [r"(?=a)b", r"(a)\1", "(", &"a".repeat(257)] {
            assert!(
                matches!(
                    parse(&link_add("x", bad, "https://e/$1")),
                    Some(PrivCmd::Malformed(_))
                ),
                "{bad}"
            );
        }
        assert!(matches!(
            parse(&link_add("x", "a", &"h".repeat(513))),
            Some(PrivCmd::Malformed(_))
        ));
        // A pattern whose compiled size exceeds the cap.
        assert!(matches!(
            parse(&link_add("x", r"(\w{500}){500}", "https://e/")),
            Some(PrivCmd::Malformed(_))
        ));
        let mut st = PrivateState::default();
        for i in 0..9u64 {
            feed(
                &mut st,
                &link_add(&format!("r{i}"), "a", "https://e/"),
                i * 1000,
            );
        }
        assert_eq!(st.links.rules().len(), LINK_RULES_MAX);
        assert_eq!(st.counters.malformed, 1);
        let rev = st.links.rev();
        feed(&mut st, b"7726;link;clear;r0", 20_000);
        assert_eq!(st.links.rules().len(), LINK_RULES_MAX - 1);
        assert!(st.links.rev() > rev);
        feed(&mut st, b"7726;link;clear", 30_000);
        assert!(st.links.rules().is_empty());
    }

    #[test]
    fn link_rate_limit_and_gate() {
        let mut st = PrivateState::default();
        for i in 0..7 {
            feed(&mut st, &link_add(&format!("r{i}"), "a", "https://e/"), 1);
        }
        assert_eq!(st.links.rules().len(), LINK_RATE_PER_SEC as usize);
        assert_eq!(st.counters.dropped, 2);
        let mut m = MenuRegistry::default();
        let mut gate = on();
        gate.features.set(Feature::Link, false);
        let c = parse(&link_add("z", "a", "https://e/")).unwrap();
        st.handle(&gate, &mut m, c, false, 9000);
        assert_eq!(st.counters.swallowed_disabled, 1);
        let changed = st.gate_changed(&on(), &gate, &mut m);
        assert_eq!(changed, vec![Feature::Link]);
        assert!(st.links.rules().is_empty());
    }
}
