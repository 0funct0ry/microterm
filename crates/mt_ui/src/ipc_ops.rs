//! Host-independent pieces of the IPC methods (SPEC §12.2), shared by the GUI host
//! (`mux_host`) and the headless host used by the CLI end-to-end tests: mux snapshots, event
//! conversion, text extraction, pasting and `record.start`/`record.stop`.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use mt_mux::{DomainKind, Id, Mux, MuxEvent, SpawnSpec};
use mt_proto::{
    ActivateTabParams, Event, GetTextParams, MuxSnapshot, PaneInfo, RpcError, SendTextParams,
    SpawnParams, TabInfo, Text, WindowInfo, WorkspaceInfo,
};
use mt_term::Terminal;

pub fn event_of(e: &MuxEvent) -> Event {
    match *e {
        MuxEvent::PaneAdded { pane } => Event::PaneAdded { pane },
        MuxEvent::PaneRemoved { pane } => Event::PaneRemoved { pane },
        MuxEvent::PaneOutput { pane } => Event::PaneOutput { pane },
        MuxEvent::TitleChanged { pane } => Event::TitleChanged { pane },
        MuxEvent::TabAdded { window, tab } => Event::TabAdded { window, tab },
        MuxEvent::TabRemoved { window, tab } => Event::TabRemoved { window, tab },
        MuxEvent::TabMoved {
            window,
            tab,
            from,
            to,
        } => Event::TabMoved {
            window,
            tab,
            from,
            to,
        },
        MuxEvent::WindowAdded { window } => Event::WindowAdded { window },
        MuxEvent::WindowRemoved { window } => Event::WindowRemoved { window },
        MuxEvent::WorkspaceSwitched { from, to } => Event::WorkspaceSwitched { from, to },
    }
}

pub fn snapshot(mux: &Mux) -> MuxSnapshot {
    let active = mux.active_workspace().id;
    let workspaces = mux
        .workspaces()
        .iter()
        .map(|ws| WorkspaceInfo {
            id: ws.id,
            name: ws.name.clone(),
            active: ws.id == active,
            windows: mux
                .windows_in(ws.id)
                .map(|w| WindowInfo {
                    id: w.id,
                    title: w.title.clone(),
                    tabs: w
                        .tabs
                        .iter()
                        .enumerate()
                        .map(|(i, t)| TabInfo {
                            id: t.id,
                            title: t
                                .title_override
                                .clone()
                                .unwrap_or_else(|| t.pane().title.clone()),
                            active: i == w.active,
                            panes: t
                                .tree
                                .panes()
                                .into_iter()
                                .map(|p| PaneInfo {
                                    id: p.id,
                                    title: p.title.clone(),
                                    cwd: p.cwd.as_ref().map(|c| c.to_string_lossy().into_owned()),
                                    domain: "local".into(),
                                })
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect();
    MuxSnapshot { workspaces }
}

/// The launch spec of a `pane.spawn`. Only the local domain exists until M10.
pub fn spawn_spec(p: &SpawnParams) -> Result<SpawnSpec, RpcError> {
    if let Some(d) = p.domain.as_deref() {
        if d != "local" {
            return Err(RpcError::not_found(format!("no domain named \"{d}\"")));
        }
    }
    let command = p.command.clone().filter(|c| !c.is_empty());
    Ok(SpawnSpec {
        cwd: p.cwd.as_deref().map(expand_tilde),
        command,
        title: p.title.clone(),
    })
}

pub fn expand_tilde(path: &str) -> std::path::PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => Path::new(&home).join(rest),
        _ if path == "~" => std::env::var_os("HOME").map_or_else(|| path.into(), Into::into),
        _ => path.into(),
    }
}

pub fn domain_name(kind: &DomainKind) -> String {
    match kind {
        DomainKind::Local => "local".into(),
        DomainKind::Ssh { name } | DomainKind::Serial { name } => name.clone(),
    }
}

/// `(window, tab index)` a `tab.activate` refers to. `default_window` is used when the request
/// names no window.
pub fn resolve_activation(
    mux: &Mux,
    p: &ActivateTabParams,
    default_window: Option<Id>,
) -> Result<(Id, usize), RpcError> {
    let window = |p: &ActivateTabParams| {
        p.window
            .or(default_window)
            .ok_or_else(|| RpcError::not_found("no window"))
    };
    if let Some(tab) = p.tab_id {
        return mux
            .tab_location(tab)
            .ok_or_else(|| RpcError::not_found(format!("tab {tab} does not exist")));
    }
    let w = window(p)?;
    let win = mux
        .window(w)
        .ok_or_else(|| RpcError::not_found(format!("window {w} does not exist")))?;
    match (p.tab_index, p.tab_relative) {
        (Some(i), _) if i < win.tabs.len() => Ok((w, i)),
        (Some(i), _) => Err(RpcError::not_found(format!("window {w} has no tab {i}"))),
        (None, Some(d)) => {
            let n = win.tabs.len() as i64;
            Ok((w, (win.active as i64 + d).rem_euclid(n.max(1)) as usize))
        }
        (None, None) => Err(RpcError::bad_request(
            "give tab_id, tab_index or tab_relative",
        )),
    }
}

/// `pane.get_text`: rows `start_line..=end_line`, where line `0` is the top visible row and
/// negative numbers reach into scrollback. Soft-wrapped rows are joined; trailing blank lines
/// are dropped.
pub fn get_text(term: &Terminal, p: &GetTextParams) -> Result<Text, RpcError> {
    let history = term.history_size() as i64;
    let (_, rows) = term.size();
    let start = p.start_line.unwrap_or(0);
    let end = p.end_line.unwrap_or(rows as i64 - 1);
    if end < start {
        return Err(RpcError::bad_request("end_line is before start_line"));
    }
    let total = term.total_lines() as i64;
    let from = (history + start).clamp(0, total) as usize;
    let to = (history + end + 1).clamp(0, total) as usize;
    let mut out = String::new();
    if p.escapes {
        let lines: Vec<String> = term
            .styled_lines(from, to)
            .iter()
            .map(|r| mt_term::encode_row(r))
            .collect();
        out = lines.join("\n");
        out.truncate(out.trim_end_matches('\n').len());
    } else {
        for row in term.lines_range(from, to) {
            if row.wrapped {
                out.extend(row.chars.iter().filter(|c| **c != '\0'));
            } else {
                out.push_str(&row.text());
                out.push('\n');
            }
        }
        out.truncate(out.trim_end_matches('\n').len());
    }
    if !out.is_empty() {
        out.push('\n');
    }
    Ok(Text { text: out })
}

/// `pane.send_text`: pasted (bracketed when the program asked for it) unless `paste` is off,
/// in which case the bytes are typed as they are.
pub fn send_text(term: &Terminal, p: &SendTextParams) {
    if p.paste {
        term.paste(&p.text);
    } else {
        term.write(p.text.clone().into_bytes());
    }
}

struct Recording {
    done: Option<JoinHandle<()>>,
}

/// Active `record.start` sessions, keyed by pane id.
#[derive(Default)]
pub struct Recorders {
    active: HashMap<Id, Recording>,
}

impl Recorders {
    pub fn start(
        &mut self,
        pane: Id,
        term: &Arc<Terminal>,
        path: &str,
        input: bool,
    ) -> Result<(), RpcError> {
        self.active
            .retain(|_, r| r.done.as_ref().is_some_and(|h| !h.is_finished()));
        if self.active.contains_key(&pane) {
            return Err(RpcError::bad_request(format!(
                "pane {pane} is already being recorded"
            )));
        }
        let (cols, rows) = term.size();
        let file = File::create(path)
            .map_err(|e| RpcError::bad_request(format!("cannot create {path}: {e}")))?;
        let mut header = mt_record::Header::new(cols as u16, rows as u16);
        header.timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs());
        for k in ["TERM", "SHELL"] {
            if let Ok(v) = std::env::var(k) {
                header.env.insert(k.into(), v);
            }
        }
        let mut writer = mt_record::Writer::new(BufWriter::new(file), &header)
            .map_err(|e| RpcError::internal(e.to_string()))?;
        let (otx, orx) = mpsc::channel::<Vec<u8>>();
        let (itx, irx) = mpsc::channel::<Vec<u8>>();
        let t0 = Instant::now();
        // One thread drains both streams in arrival order.
        let (merged_tx, merged_rx) = mpsc::channel::<(bool, Vec<u8>)>();
        for (rx, is_input) in [(orx, false), (irx, true)] {
            let tx = merged_tx.clone();
            std::thread::spawn(move || {
                while let Ok(b) = rx.recv() {
                    if tx.send((is_input, b)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(merged_tx);
        let handle = std::thread::Builder::new()
            .name("microterm-record".into())
            .spawn(move || {
                let mut out_join = mt_record::Utf8Joiner::default();
                let mut in_join = mt_record::Utf8Joiner::default();
                while let Ok((is_input, bytes)) = merged_rx.recv() {
                    let t = t0.elapsed().as_secs_f64();
                    let (join, write): (_, fn(&mut _, f64, &[u8]) -> _) = if is_input {
                        (
                            &mut in_join,
                            |w: &mut mt_record::Writer<BufWriter<File>>, t, b| w.input(t, b),
                        )
                    } else {
                        (&mut out_join, |w, t, b| w.output(t, b))
                    };
                    let chunk = join.push(&bytes);
                    if !chunk.is_empty() {
                        let _ = write(&mut writer, t, &chunk);
                    }
                }
                let t = t0.elapsed().as_secs_f64();
                let rest = out_join.finish();
                if !rest.is_empty() {
                    let _ = writer.output(t, &rest);
                }
                let _ = writer.flush();
            })
            .map_err(|e| RpcError::internal(e.to_string()))?;
        term.set_output_tap(Some(otx));
        term.set_input_tap(input.then_some(itx));
        self.active.insert(pane, Recording { done: Some(handle) });
        Ok(())
    }

    /// Ends the recording and waits until the file is complete. Succeeds silently when `term`
    /// is gone (the pane exited and the recording finished by itself).
    pub fn stop(&mut self, pane: Id, term: Option<&Terminal>) -> Result<(), RpcError> {
        let Some(mut rec) = self.active.remove(&pane) else {
            return Err(RpcError::not_found(format!(
                "pane {pane} is not being recorded"
            )));
        };
        if let Some(t) = term {
            t.set_output_tap(None);
            t.set_input_tap(None);
        }
        if let Some(h) = rec.done.take() {
            let _ = h.join();
        }
        Ok(())
    }

    /// Like [`Recorders::stop`], for a pane that is closing; ignores panes not recorded.
    pub fn finish(&mut self, pane: Id, term: Option<&Terminal>) {
        let _ = self.stop(pane, term);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_lists_the_tree_and_titles() {
        let mut mux = Mux::new();
        let w = mux.spawn_window(None);
        let (tab, pane) = mux.spawn_tab(w, None, Some("srv".into())).unwrap();
        mux.set_pane_title(pane, "zsh");
        let s = snapshot(&mux);
        let ws = &s.workspaces[0];
        assert!(ws.active);
        assert_eq!(ws.windows[0].tabs[0].id, tab);
        assert_eq!(ws.windows[0].tabs[0].panes[0].id, pane);
        assert_eq!(ws.windows[0].tabs[0].panes[0].title, "zsh");
    }

    #[test]
    fn activation_resolves_ids_indexes_and_relative() {
        let mut mux = Mux::new();
        let w = mux.spawn_window(None);
        let tabs: Vec<_> = (0..3)
            .map(|_| mux.spawn_tab(w, None, None).unwrap().0)
            .collect();
        let by = |p: ActivateTabParams| resolve_activation(&mux, &p, Some(w));
        assert_eq!(
            by(ActivateTabParams {
                tab_id: Some(tabs[1]),
                ..Default::default()
            })
            .unwrap(),
            (w, 1)
        );
        assert_eq!(
            by(ActivateTabParams {
                tab_index: Some(2),
                ..Default::default()
            })
            .unwrap(),
            (w, 2)
        );
        assert!(by(ActivateTabParams {
            tab_index: Some(9),
            ..Default::default()
        })
        .is_err());
        // Spawning left tab 2 active; -1 goes to 1 and +1 wraps to 0.
        assert_eq!(
            by(ActivateTabParams {
                tab_relative: Some(-1),
                ..Default::default()
            })
            .unwrap(),
            (w, mux.window(w).unwrap().active.saturating_sub(1))
        );
        assert_eq!(
            by(ActivateTabParams {
                tab_relative: Some(1),
                ..Default::default()
            })
            .unwrap()
            .1,
            (mux.window(w).unwrap().active + 1) % 3
        );
    }

    #[test]
    fn only_the_local_domain_exists() {
        let ok = SpawnParams::default();
        assert!(spawn_spec(&ok).is_ok());
        let bad = SpawnParams {
            domain: Some("prod".into()),
            ..Default::default()
        };
        assert_eq!(
            spawn_spec(&bad).unwrap_err().code,
            mt_proto::ErrorCode::NotFound
        );
    }
}
