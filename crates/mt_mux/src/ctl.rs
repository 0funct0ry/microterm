//! The model side of `microterm ctl` (SPEC §12.1–§12.3): plain snapshot rows of the mux,
//! selector evaluation, target resolution and the `confirm_close` decision for scripted closes.
//!
//! Everything here is GPUI-free and works on [`Rows`], a flat copy of the mux plus the live
//! facts only a host knows (foreground process, size), so it is unit-testable without a UI.

use mt_proto::selector::{Key, Resource, Selector};
use mt_proto::{
    LayoutNode, MuxSnapshot, PaneInfo, PaneStatus, RpcError, Size, SplitAxis, TabInfo, TabNode,
    Target, WindowInfo, WindowNode, WorkspaceInfo, WorkspaceNode,
};

use crate::domain::DomainKind;
use crate::model::{Axis, Error, Id, Mux, PaneTree};
use crate::title::{should_confirm_close, ConfirmClose};

impl From<Error> for RpcError {
    fn from(e: Error) -> Self {
        match e {
            Error::NoWindow(_)
            | Error::NoTab(_)
            | Error::NoPane(_)
            | Error::NoWorkspace(_)
            | Error::NoDomain(_) => RpcError::not_found(e.to_string()),
            _ => RpcError::bad_request(e.to_string()),
        }
    }
}

/// What a host knows about a pane that the model does not.
#[derive(Debug, Clone, Default)]
pub struct Live {
    pub process: Option<String>,
    pub size: (u16, u16),
    pub exited: bool,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneRow {
    pub id: Id,
    pub tab: Id,
    pub window: Id,
    pub workspace: String,
    pub domain: String,
    pub title: String,
    pub cwd: Option<String>,
    pub process: Option<String>,
    pub size: (u16, u16),
    pub focused: bool,
    pub active: bool,
    pub zoomed: bool,
    pub exited: bool,
    pub exit_code: Option<i32>,
    pub status: Option<PaneStatus>,
    /// Resolved color of the pane's tab (name or `#rrggbb`).
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabRow {
    pub id: Id,
    pub window: Id,
    pub workspace: String,
    pub domain: String,
    pub title: String,
    pub active: bool,
    pub focused: bool,
    pub panes: Vec<Id>,
    pub layout: LayoutNode,
    /// Resolved color (name or `#rrggbb`) and its source (`manual|declared|rule`).
    pub color: Option<String>,
    pub color_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRow {
    pub id: Id,
    pub workspace: String,
    pub title: String,
    pub active: bool,
    pub focused: bool,
    pub tabs: Vec<Id>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRow {
    pub id: Id,
    pub name: String,
    pub active: bool,
    pub windows: Vec<Id>,
}

/// A flat snapshot of the whole mux.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rows {
    pub workspaces: Vec<WorkspaceRow>,
    pub windows: Vec<WindowRow>,
    pub tabs: Vec<TabRow>,
    pub panes: Vec<PaneRow>,
    pub focused_window: Option<Id>,
    pub focused_tab: Option<Id>,
    pub focused_pane: Option<Id>,
}

/// The wire form of a pane tree.
pub fn layout_node(tree: &PaneTree) -> LayoutNode {
    match tree {
        PaneTree::Leaf(p) => LayoutNode::Leaf { pane: p.id },
        PaneTree::Split { axis, ratio, a, b } => LayoutNode::Split {
            axis: match axis {
                Axis::Horizontal => SplitAxis::Horizontal,
                Axis::Vertical => SplitAxis::Vertical,
            },
            percent: ((ratio * 100.0).round() as i64).clamp(1, 99) as u8,
            a: Box::new(layout_node(a)),
            b: Box::new(layout_node(b)),
        },
    }
}

pub fn domain_name(kind: &DomainKind) -> String {
    match kind {
        DomainKind::Local => "local".into(),
        DomainKind::Ssh { name } | DomainKind::Serial { name } => name.clone(),
    }
}

impl Mux {
    /// Snapshot of the mux. `live` supplies per-pane facts from the host's terminals.
    pub fn rows(&self, live: &dyn Fn(Id) -> Live) -> Rows {
        let active_ws = self.active_workspace().id;
        let focused_window = self.focused_window();
        let focused_tab = focused_window
            .and_then(|w| self.window(w))
            .and_then(|w| w.active_tab())
            .map(|t| t.id);
        let focused_pane = focused_window
            .and_then(|w| self.window(w))
            .and_then(|w| w.active_tab())
            .map(|t| t.pane().id);
        let ws_name = |id: Id| {
            self.workspaces()
                .iter()
                .find(|w| w.id == id)
                .map(|w| w.name.clone())
                .unwrap_or_default()
        };
        let mut rows = Rows {
            focused_window,
            focused_tab,
            focused_pane,
            ..Default::default()
        };
        for ws in self.workspaces() {
            rows.workspaces.push(WorkspaceRow {
                id: ws.id,
                name: ws.name.clone(),
                active: ws.id == active_ws,
                windows: self.windows_in(ws.id).map(|w| w.id).collect(),
            });
        }
        for w in self.windows() {
            let workspace = ws_name(w.workspace_id);
            rows.windows.push(WindowRow {
                id: w.id,
                workspace: workspace.clone(),
                title: w.title.clone(),
                active: w.workspace_id == active_ws,
                focused: Some(w.id) == focused_window,
                tabs: w.tabs.iter().map(|t| t.id).collect(),
            });
            for (i, t) in w.tabs.iter().enumerate() {
                let pane = t.pane();
                let domain = self
                    .domain_kind(pane.domain_id)
                    .map_or_else(|| "local".into(), |k| domain_name(&k));
                let l = live(pane.id);
                let title = t
                    .title_override
                    .clone()
                    .filter(|s| !s.is_empty())
                    .or_else(|| (!pane.title.is_empty()).then(|| pane.title.clone()))
                    .or_else(|| l.process.clone())
                    .unwrap_or_default();
                let (tab_color, source) = t.resolved_color();
                let color = tab_color.map(|c| c.to_string());
                let color_source = tab_color.map(|_| source.name().to_string());
                let active = i == w.active;
                let focused = active && Some(w.id) == focused_window;
                rows.tabs.push(TabRow {
                    id: t.id,
                    window: w.id,
                    workspace: workspace.clone(),
                    domain: domain.clone(),
                    title: title.clone(),
                    active,
                    focused,
                    panes: t.tree.panes().iter().map(|p| p.id).collect(),
                    layout: layout_node(&t.tree),
                    color: color.clone(),
                    color_source,
                });
                for p in t.tree.panes() {
                    let l = live(p.id);
                    rows.panes.push(PaneRow {
                        id: p.id,
                        tab: t.id,
                        window: w.id,
                        workspace: workspace.clone(),
                        domain: domain.clone(),
                        title: if p.title.is_empty() {
                            title.clone()
                        } else {
                            p.title.clone()
                        },
                        cwd: p.cwd.as_ref().map(|c| c.to_string_lossy().into_owned()),
                        process: l.process,
                        size: l.size,
                        focused: focused && p.id == pane.id,
                        active,
                        zoomed: t.zoomed == Some(p.id),
                        exited: l.exited,
                        exit_code: l.exit_code,
                        status: p.status.as_ref().map(|s| s.to_wire()),
                        color: color.clone(),
                    });
                }
            }
        }
        rows
    }
}

// ---- infos and the tree ----------------------------------------------------------------------

impl PaneRow {
    pub fn info(&self) -> PaneInfo {
        PaneInfo {
            id: self.id,
            tab: self.tab,
            window: self.window,
            workspace: self.workspace.clone(),
            domain: self.domain.clone(),
            title: self.title.clone(),
            cwd: self.cwd.clone(),
            process: self.process.clone(),
            size: Size {
                cols: self.size.0,
                rows: self.size.1,
            },
            focused: self.focused,
            zoomed: self.zoomed,
            exited: self.exited,
            exit_code: self.exit_code,
            status: self.status.clone(),
        }
    }
}

impl TabRow {
    pub fn info(&self) -> TabInfo {
        TabInfo {
            id: self.id,
            title: self.title.clone(),
            window: self.window,
            workspace: self.workspace.clone(),
            domain: self.domain.clone(),
            active: self.active,
            focused: self.focused,
            panes: self.panes.clone(),
            layout: Some(self.layout.clone()),
            color: self.color.clone(),
            color_source: self.color_source.clone(),
        }
    }
}

impl WindowRow {
    pub fn info(&self) -> WindowInfo {
        WindowInfo {
            id: self.id,
            title: self.title.clone(),
            workspace: self.workspace.clone(),
            active: self.active,
            focused: self.focused,
            tabs: self.tabs.clone(),
        }
    }
}

impl WorkspaceRow {
    pub fn info(&self) -> WorkspaceInfo {
        WorkspaceInfo {
            id: self.id,
            name: self.name.clone(),
            active: self.active,
            windows: self.windows.clone(),
        }
    }
}

impl Rows {
    /// `mux.list`: workspaces → windows → tabs → panes.
    pub fn tree(&self) -> MuxSnapshot {
        let workspaces = self
            .workspaces
            .iter()
            .map(|ws| WorkspaceNode {
                id: ws.id,
                name: ws.name.clone(),
                active: ws.active,
                windows: self
                    .windows
                    .iter()
                    .filter(|w| ws.windows.contains(&w.id))
                    .map(|w| WindowNode {
                        id: w.id,
                        title: w.title.clone(),
                        active: w.active,
                        focused: w.focused,
                        tabs: self
                            .tabs
                            .iter()
                            .filter(|t| t.window == w.id)
                            .map(|t| TabNode {
                                id: t.id,
                                title: t.title.clone(),
                                active: t.active,
                                focused: t.focused,
                                panes: self
                                    .panes
                                    .iter()
                                    .filter(|p| p.tab == t.id)
                                    .map(PaneRow::info)
                                    .collect(),
                                layout: Some(t.layout.clone()),
                                color: t.color.clone(),
                                color_source: t.color_source.clone(),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect();
        MuxSnapshot { workspaces }
    }

    pub fn workspace_by_name(&self, name: &str) -> Option<&WorkspaceRow> {
        self.workspaces.iter().find(|w| w.name == name)
    }
}

// ---- selector evaluation -----------------------------------------------------------------------

/// A selectable item.
pub trait Row {
    const RESOURCE: Resource;
    fn id(&self) -> Id;
    /// One line for ambiguity errors.
    fn describe(&self) -> String;
    /// The value `key` is matched against; `None` when the item has no such value.
    fn field(&self, key: Key) -> Option<String>;
    fn window_id(&self) -> Option<Id> {
        None
    }
    fn tab_id(&self) -> Option<Id> {
        None
    }
}

fn quote(title: &str) -> String {
    if title.is_empty() {
        String::new()
    } else {
        format!(" {title:?}")
    }
}

impl Row for PaneRow {
    const RESOURCE: Resource = Resource::Pane;
    fn id(&self) -> Id {
        self.id
    }
    fn describe(&self) -> String {
        format!(
            "pane {}{} (window {}, tab {})",
            self.id,
            quote(&self.title),
            self.window,
            self.tab
        )
    }
    fn field(&self, key: Key) -> Option<String> {
        match key {
            Key::Id => Some(self.id.to_string()),
            Key::Title => Some(self.title.clone()),
            Key::Cwd => self.cwd.clone(),
            Key::Cmd => self.process.clone(),
            Key::Domain => Some(self.domain.clone()),
            Key::Workspace => Some(self.workspace.clone()),
            Key::Window => Some(self.window.to_string()),
            Key::Tab => Some(self.tab.to_string()),
            Key::Focused => Some(self.focused.to_string()),
            Key::Active => Some(self.active.to_string()),
            Key::Status => Some(
                self.status
                    .as_ref()
                    .map_or("idle", |s| s.state.as_str())
                    .to_string(),
            ),
            Key::Exited => Some(self.exited.to_string()),
            Key::Color => self.color.clone(),
        }
    }
    fn window_id(&self) -> Option<Id> {
        Some(self.window)
    }
    fn tab_id(&self) -> Option<Id> {
        Some(self.tab)
    }
}

impl Row for TabRow {
    const RESOURCE: Resource = Resource::Tab;
    fn id(&self) -> Id {
        self.id
    }
    fn describe(&self) -> String {
        format!(
            "tab {}{} (window {})",
            self.id,
            quote(&self.title),
            self.window
        )
    }
    fn field(&self, key: Key) -> Option<String> {
        match key {
            Key::Id => Some(self.id.to_string()),
            Key::Title => Some(self.title.clone()),
            Key::Domain => Some(self.domain.clone()),
            Key::Color => self.color.clone(),
            Key::Workspace => Some(self.workspace.clone()),
            Key::Window => Some(self.window.to_string()),
            Key::Focused => Some(self.focused.to_string()),
            Key::Active => Some(self.active.to_string()),
            _ => None,
        }
    }
    fn window_id(&self) -> Option<Id> {
        Some(self.window)
    }
}

impl Row for WindowRow {
    const RESOURCE: Resource = Resource::Window;
    fn id(&self) -> Id {
        self.id
    }
    fn describe(&self) -> String {
        format!(
            "window {}{} (workspace {})",
            self.id,
            quote(&self.title),
            self.workspace
        )
    }
    fn field(&self, key: Key) -> Option<String> {
        match key {
            Key::Id => Some(self.id.to_string()),
            Key::Title => Some(self.title.clone()),
            Key::Workspace => Some(self.workspace.clone()),
            Key::Focused => Some(self.focused.to_string()),
            Key::Active => Some(self.active.to_string()),
            _ => None,
        }
    }
}

impl Row for WorkspaceRow {
    const RESOURCE: Resource = Resource::Workspace;
    fn id(&self) -> Id {
        self.id
    }
    fn describe(&self) -> String {
        format!("workspace {} {:?}", self.id, self.name)
    }
    fn field(&self, key: Key) -> Option<String> {
        (key == Key::Id).then(|| self.id.to_string())
    }
}

/// Whether `row` satisfies every clause. A missing value fails a positive clause and satisfies
/// a negated one.
pub fn matches<R: Row>(sel: &Selector, row: &R) -> bool {
    sel.clauses.iter().all(|c| {
        let hit = row.field(c.key).is_some_and(|v| c.pattern.is_match(&v));
        hit != c.negate
    })
}

/// Parses `expr`, checks its keys against `R` and returns the ids of the matching rows.
pub fn evaluate<R: Row>(expr: &str, rows: &[R]) -> Result<Vec<Id>, RpcError> {
    let sel = Selector::parse(expr)?;
    sel.check(R::RESOURCE)?;
    Ok(rows
        .iter()
        .filter(|r| matches(&sel, *r))
        .map(Row::id)
        .collect())
}

fn not_found<R: Row>(id: Id) -> RpcError {
    RpcError::not_found(format!("{} {id} does not exist", R::RESOURCE.name()))
}

/// Rows a `list` verb returns: the explicit id, the `--match` hits, or everything; narrowed by
/// the `window`/`tab` filters of the target. Zero matches is not an error.
pub fn select<'a, R: Row>(target: &Target, rows: &'a [R]) -> Result<Vec<&'a R>, RpcError> {
    let mut out: Vec<&R> = match (target.id_for(R::RESOURCE), &target.expr) {
        (Some(id), _) => vec![rows
            .iter()
            .find(|r| r.id() == id)
            .ok_or_else(|| not_found::<R>(id))?],
        (None, Some(expr)) => {
            let ids = evaluate(expr, rows)?;
            rows.iter().filter(|r| ids.contains(&r.id())).collect()
        }
        (None, None) => rows.iter().collect(),
    };
    if R::RESOURCE != Resource::Window {
        if let Some(w) = target.window {
            out.retain(|r| r.window_id() == Some(w));
        }
    }
    if R::RESOURCE == Resource::Pane {
        if let Some(t) = target.tab {
            out.retain(|r| r.tab_id() == Some(t));
        }
    }
    Ok(out)
}

/// How a verb treats a missing target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// `get`, `get-text`: falls back to the focused item.
    ReadOnly,
    /// `send-text`, `close`, `rename`, …: never falls back to the focused item.
    Acting { all_allowed: bool },
}

/// Server half of SPEC §12.3 target resolution. Steps 1–3 (explicit id, `--match`, caller
/// environment) arrive as `target`; this applies them and the focused fallback and enforces
/// the cardinality rules: zero matches → not found, several → ambiguous unless `--all`.
pub fn resolve<R: Row>(
    target: &Target,
    rows: &[R],
    focused: Option<Id>,
    verb: Verb,
) -> Result<Vec<Id>, RpcError> {
    let res = R::RESOURCE.name();
    if let Some(id) = target.id_for(R::RESOURCE) {
        return rows
            .iter()
            .any(|r| r.id() == id)
            .then_some(vec![id])
            .ok_or_else(|| not_found::<R>(id));
    }
    if let Some(expr) = &target.expr {
        let ids = evaluate(expr, rows)?;
        let all_ok = target.all && matches!(verb, Verb::Acting { all_allowed: true });
        return match ids.len() {
            0 => Err(RpcError::not_found(format!("no {res} matches {expr:?}"))),
            1 => Ok(ids),
            n if all_ok => {
                let _ = n;
                Ok(ids)
            }
            n => Err(RpcError::ambiguous(
                format!("{n} {res}s match {expr:?}; narrow the selector or pass --all"),
                rows.iter()
                    .filter(|r| ids.contains(&r.id()))
                    .map(Row::describe)
                    .collect(),
            )),
        };
    }
    match verb {
        Verb::ReadOnly => focused
            .filter(|f| rows.iter().any(|r| r.id() == *f))
            .map(|f| vec![f])
            .ok_or_else(|| RpcError::not_found(format!("no focused {res}"))),
        Verb::Acting { .. } => Err(RpcError::bad_request(format!(
            "no {res} given: pass --{res} ID or --match EXPR, or run inside the {res} \
             ($MICROTERM_{})",
            res.to_uppercase()
        ))),
    }
}

/// Like [`resolve`] for verbs that need exactly one item.
pub fn resolve_one<R: Row>(
    target: &Target,
    rows: &[R],
    focused: Option<Id>,
    verb: Verb,
) -> Result<Id, RpcError> {
    let ids = resolve(target, rows, focused, verb)?;
    ids.first()
        .copied()
        .ok_or_else(|| RpcError::not_found(format!("no {}", R::RESOURCE.name())))
}

// ---- scripted closes ---------------------------------------------------------------------------

/// Whether `confirm_close` would have asked before closing something holding panes whose
/// "busy" flags (a non-shell foreground process) are given. Scripted closes never show the
/// dialog: without `--force` a `true` here becomes exit code 8.
pub fn close_would_prompt(policy: ConfirmClose, busy: impl IntoIterator<Item = bool>) -> bool {
    should_confirm_close(policy, busy.into_iter().any(|b| b))
}

/// The error of a refused scripted close.
pub fn confirm_required(what: &str) -> RpcError {
    RpcError::confirm_required(format!(
        "confirm_close would ask before closing {what}; pass --force to close it anyway \
         (nothing was closed)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mt_proto::ErrorCode;

    fn pane(id: Id, title: &str, cwd: Option<&str>, process: Option<&str>) -> PaneRow {
        PaneRow {
            id,
            tab: id + 100,
            window: if id < 3 { 1 } else { 2 },
            workspace: if id < 3 { "api".into() } else { "infra".into() },
            domain: if id == 4 {
                "prod".into()
            } else {
                "local".into()
            },
            title: title.into(),
            cwd: cwd.map(Into::into),
            process: process.map(Into::into),
            size: (80, 24),
            focused: id == 1,
            active: id != 2,
            zoomed: false,
            exited: id == 5,
            exit_code: None,
            status: (id == 2).then(|| PaneStatus {
                state: mt_proto::StatusState::Working,
                text: "running tests".into(),
                progress: Some(40),
                updated_at: "2026-10-18T09:14:03.000Z".into(),
            }),
            color: (id == 4).then(|| "red".into()),
        }
    }

    fn panes() -> Vec<PaneRow> {
        vec![
            pane(1, "zsh", Some("/home/me/src/api"), None),
            pane(
                2,
                "build api",
                Some("/home/me/src"),
                Some("cargo watch -x test"),
            ),
            pane(3, "build web", Some("/tmp"), Some("npm run build")),
            pane(4, "prod shell", None, None),
            pane(5, "done", Some("/tmp"), None),
        ]
    }

    fn ids(expr: &str) -> Vec<Id> {
        evaluate(expr, &panes()).unwrap()
    }

    #[test]
    fn every_pane_key_matches() {
        assert_eq!(ids("id:3"), [3]);
        assert_eq!(ids("title:build*"), [2, 3]);
        assert_eq!(ids("cwd:/tmp"), [3, 5]);
        assert_eq!(ids("cmd:cargo*"), [2]);
        assert_eq!(ids("domain:prod"), [4]);
        assert_eq!(ids("workspace:infra"), [3, 4, 5]);
        assert_eq!(ids("window:1"), [1, 2]);
        assert_eq!(ids("tab:103"), [3]);
        assert_eq!(ids("focused:true"), [1]);
        assert_eq!(ids("active:false"), [2]);
        assert_eq!(ids("status:working"), [2]);
        assert_eq!(ids("status:idle"), [1, 3, 4, 5], "an unset status is idle");
        assert_eq!(ids("exited:true"), [5]);
        assert_eq!(ids("color:red"), [4]);
        assert_eq!(
            ids("!color:red"),
            [1, 2, 3, 5],
            "no color never matches a positive clause"
        );
    }

    #[test]
    fn glob_regex_negation_and_and() {
        assert_eq!(ids("title:re:^build (api|web)$"), [2, 3]);
        assert_eq!(ids("title:build*,!workspace:infra"), [2]);
        assert_eq!(ids("!exited:true,domain:local,workspace:infra"), [3]);
        assert_eq!(ids("cmd:re:cargo|npm"), [2, 3]);
        // A missing value never matches a positive clause and always satisfies a negated one.
        assert_eq!(ids("!cwd:/tmp"), [1, 2, 4]);
        assert!(ids("cwd:*").iter().all(|id| *id != 4));
    }

    #[test]
    fn tilde_in_cwd_expands() {
        std::env::set_var("HOME", "/home/me");
        assert_eq!(ids("cwd:~/src"), [2]);
        assert_eq!(ids("cwd:~/src/*"), [1]);
    }

    #[test]
    fn inapplicable_keys_are_usage_errors() {
        let e = evaluate(
            "cwd:/tmp",
            &[TabRow {
                id: 1,
                window: 1,
                workspace: "a".into(),
                domain: "local".into(),
                title: "t".into(),
                active: true,
                focused: true,
                panes: vec![],
                layout: LayoutNode::Leaf { pane: 1 },
                color: None,
                color_source: None,
            }],
        )
        .unwrap_err();
        assert_eq!(e.code, ErrorCode::BadRequest);
        let w = [WindowRow {
            id: 1,
            workspace: "a".into(),
            title: String::new(),
            active: true,
            focused: true,
            tabs: vec![],
        }];
        assert!(evaluate("domain:local", &w).is_err());
        assert!(evaluate("tab:1", &w).is_err());
        assert_eq!(evaluate("focused:true", &w).unwrap(), [1]);
    }

    #[test]
    fn explicit_id_beats_match_and_must_exist() {
        let rows = panes();
        let t = Target {
            pane: Some(3),
            expr: Some("title:zsh".into()),
            ..Default::default()
        };
        let verb = Verb::Acting { all_allowed: false };
        assert_eq!(resolve(&t, &rows, Some(1), verb).unwrap(), [3]);
        let e = resolve(&Target::pane(99), &rows, None, verb).unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
    }

    #[test]
    fn match_cardinality() {
        let rows = panes();
        let verb = Verb::Acting { all_allowed: false };
        let e = resolve(&Target::matching("title:nope"), &rows, None, verb).unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
        let e = resolve(&Target::matching("title:build*"), &rows, None, verb).unwrap_err();
        assert_eq!(e.code, ErrorCode::Ambiguous);
        assert_eq!(e.candidates.len(), 2);
        assert!(e.candidates[0].starts_with("pane 2"));
        // --all is honoured only where the verb allows it.
        let all = Target {
            all: true,
            ..Target::matching("title:build*")
        };
        assert_eq!(
            resolve(&all, &rows, None, verb).unwrap_err().code,
            ErrorCode::Ambiguous
        );
        let verb = Verb::Acting { all_allowed: true };
        assert_eq!(resolve(&all, &rows, None, verb).unwrap(), [2, 3]);
        assert_eq!(
            resolve(&Target::matching("title:zsh"), &rows, None, verb).unwrap(),
            [1]
        );
    }

    #[test]
    fn focused_fallback_is_read_only_only() {
        let rows = panes();
        let none = Target::default();
        assert_eq!(resolve(&none, &rows, Some(1), Verb::ReadOnly).unwrap(), [1]);
        assert_eq!(
            resolve(&none, &rows, None, Verb::ReadOnly)
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        for all_allowed in [false, true] {
            let e = resolve(&none, &rows, Some(1), Verb::Acting { all_allowed }).unwrap_err();
            assert_eq!(
                e.code,
                ErrorCode::BadRequest,
                "never falls back to the focused pane"
            );
        }
    }

    #[test]
    fn select_filters_by_parents_and_match() {
        let rows = panes();
        let t = Target {
            window: Some(2),
            ..Default::default()
        };
        let got: Vec<Id> = select(&t, &rows).unwrap().iter().map(|r| r.id).collect();
        assert_eq!(got, [3, 4, 5]);
        let got = select(&Target::matching("title:nope"), &rows).unwrap();
        assert!(got.is_empty());
        assert_eq!(select(&Target::default(), &rows).unwrap().len(), 5);
    }

    #[test]
    fn close_matrix_by_policy() {
        use ConfirmClose::*;
        for (policy, busy, prompts) in [
            (Always, false, true),
            (Always, true, true),
            (Process, false, false),
            (Process, true, true),
            (Never, false, false),
            (Never, true, false),
        ] {
            assert_eq!(
                close_would_prompt(policy, [busy]),
                prompts,
                "{policy:?} {busy}"
            );
        }
        assert!(close_would_prompt(Process, [false, true]));
        assert!(!close_would_prompt(Process, []));
        assert_eq!(
            confirm_required("tab 3").code.exit_code(),
            mt_proto::exit::CONFIRM_REQUIRED
        );
    }
}
