//! `multiplexing = "none"` (SPEC §11.3 fallback): plain SSH shell sessions behind the same mux
//! protocol, so the link and the UI have one code path. A tiny in-process "daemon" speaks the
//! protocol on one end of a pipe and opens an SSH channel with a PTY per pane on the other.
//! There is nothing to re-attach to after a disconnect: panes simply end.

use std::collections::HashMap;
use std::sync::Arc;

use mt_proto::mux::{self, Decoder, ExitInfo, Msg, PaneInfo};
use russh::client::Msg as ClientMsg;
use russh::{ChannelMsg, ChannelWriteHalf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::link::Duplex;
use crate::session::Connection;

struct PlainPane {
    write: ChannelWriteHalf<ClientMsg>,
}

/// Quotes one argument for the remote shell.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The client end of the pipe; the shim task owns the other end and `conn`.
pub fn stream(conn: Arc<Connection>) -> Box<dyn Duplex> {
    let (client, server) = tokio::io::duplex(1 << 20);
    tokio::spawn(serve(server, conn));
    Box::new(client)
}

async fn serve(io: tokio::io::DuplexStream, conn: Arc<Connection>) {
    let (mut r, mut w) = tokio::io::split(io);
    let (out_tx, mut out_rx) = unbounded_channel::<Msg>();
    let writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            let Ok(bytes) = mux::encode(&m) else { continue };
            if w.write_all(&bytes).await.is_err() {
                break;
            }
        }
    });
    let mut panes: HashMap<u64, PlainPane> = HashMap::new();
    let mut next_id = 1u64;
    let mut dec = Decoder::default();
    let mut buf = vec![0u8; 64 * 1024];
    'outer: loop {
        // The SSH connection can die without this pipe noticing; ending the pipe is what tells
        // the link to reconnect.
        let n = loop {
            if conn.is_closed() {
                break 'outer;
            }
            tokio::select! {
                r = r.read(&mut buf) => match r {
                    Ok(0) | Err(_) => break 'outer,
                    Ok(n) => break n,
                },
                () = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            }
        };
        dec.push(&buf[..n]);
        while let Ok(Some(msg)) = dec.next_msg() {
            match msg {
                Msg::Hello { .. } => {
                    let _ = out_tx.send(mux::hello());
                }
                Msg::Attach { .. } => {
                    let _ = out_tx.send(Msg::Attached {
                        panes: Vec::<PaneInfo>::new(),
                    });
                }
                Msg::Ping(n) => {
                    let _ = out_tx.send(Msg::Pong(n));
                }
                Msg::Spawn(req) => {
                    let id = next_id;
                    next_id += 1;
                    match open_pane(&conn, &req).await {
                        Ok((p, read)) => {
                            panes.insert(id, p);
                            // `Spawned` must reach the client before any output, or the first
                            // output (the prompt) has no pane to go to and is lost.
                            let _ = out_tx.send(Msg::Spawned {
                                req_id: req.req_id,
                                pane: id,
                            });
                            forward(id, read, out_tx.clone());
                        }
                        Err(message) => {
                            // Show the failure in the tab instead of leaving it blank.
                            let _ = out_tx.send(Msg::Spawned {
                                req_id: req.req_id,
                                pane: id,
                            });
                            let text = format!("\r\n[could not open a shell: {message}]\r\n");
                            let _ = out_tx.send(Msg::Output {
                                pane: id,
                                seq: text.len() as u64,
                                bytes: text.into_bytes(),
                            });
                            let _ = out_tx.send(Msg::PaneExited {
                                pane: id,
                                status: None,
                            });
                        }
                    }
                }
                Msg::Input { pane, bytes } => {
                    if let Some(p) = panes.get(&pane) {
                        let _ = p.write.data_bytes(bytes).await;
                    }
                }
                Msg::Resize { pane, cols, rows } => {
                    if let Some(p) = panes.get(&pane) {
                        let _ = p.write.window_change(cols as u32, rows as u32, 0, 0).await;
                    }
                }
                Msg::Kill { pane } => {
                    if let Some(p) = panes.remove(&pane) {
                        let _ = p.write.close().await;
                    }
                }
                Msg::Detach => break 'outer,
                _ => {}
            }
        }
    }
    for (_, p) in panes.drain() {
        let _ = p.write.close().await;
    }
    drop(out_tx);
    let _ = writer.await;
}

async fn open_pane(
    conn: &Connection,
    req: &mux::SpawnReq,
) -> Result<(PlainPane, russh::ChannelReadHalf), String> {
    let ch = conn
        .handle
        .channel_open_session()
        .await
        .map_err(|e| e.to_string())?;
    let (read, write) = ch.split();
    write
        .request_pty(
            false,
            "xterm-256color",
            req.cols as u32,
            req.rows as u32,
            0,
            0,
            &[],
        )
        .await
        .map_err(|e| e.to_string())?;
    let mut cmd = String::new();
    if let Some(cwd) = &req.cwd {
        cmd.push_str(&format!("cd {} && ", crate::bootstrap::sh_path(cwd)));
    }
    match &req.command {
        Some(c) if !c.is_empty() => {
            cmd.push_str("exec ");
            cmd.push_str(
                &c.iter()
                    .map(|a| shell_quote(a))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            write
                .exec(true, cmd.into_bytes())
                .await
                .map_err(|e| e.to_string())?;
        }
        _ if req.cwd.is_some() => {
            cmd.push_str("exec \"${SHELL:-/bin/sh}\" -l");
            write
                .exec(true, cmd.into_bytes())
                .await
                .map_err(|e| e.to_string())?;
        }
        _ => write.request_shell(true).await.map_err(|e| e.to_string())?,
    }
    Ok((PlainPane { write }, read))
}

/// Pumps a channel's output into the mux stream until it closes.
fn forward(id: u64, mut read: russh::ChannelReadHalf, out: UnboundedSender<Msg>) {
    tokio::spawn(async move {
        let (mut seq, mut status) = (0u64, None);
        while let Some(m) = read.wait().await {
            match m {
                ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                    seq += data.len() as u64;
                    let _ = out.send(Msg::Output {
                        pane: id,
                        seq,
                        bytes: data.to_vec(),
                    });
                }
                ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status as i32),
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        let _ = out.send(Msg::PaneExited { pane: id, status });
        let _ = ExitInfo { status };
    });
}

#[cfg(test)]
mod tests {
    use super::shell_quote;

    #[test]
    fn quoting_survives_single_quotes() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
