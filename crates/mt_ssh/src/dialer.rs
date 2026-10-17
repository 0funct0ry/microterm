//! The real [`Dialer`]: SSH connect, bootstrap, run `microterm-mux proxy` (SPEC §11.3).

use std::sync::Arc;
use std::time::Duration;

use crate::bootstrap::{self, BootstrapError};
use crate::config::Target;
use crate::link::{DialFailure, Dialed, Dialer};
use crate::prompt::BoxFuture;
use crate::session::{self, ConnectOptions, SshError};

pub struct SshDialer {
    pub target: Target,
    pub remote_dir: String,
    /// `multiplexing = "microterm"` (persistent); `false` = plain sessions.
    pub multiplexing: bool,
    pub opts: ConnectOptions,
}

/// Failures that retrying cannot fix without the user acting.
pub fn classify(e: SshError) -> DialFailure {
    match e {
        SshError::HostKeyChanged { .. }
        | SshError::HostKeyRejected(_)
        | SshError::AuthFailed { .. }
        | SshError::Cancelled => DialFailure::Fatal(e.to_string()),
        other => DialFailure::Transient(other.to_string()),
    }
}

impl Dialer for SshDialer {
    fn dial(&self) -> BoxFuture<Result<Dialed, DialFailure>> {
        let (target, dir, mux, opts) = (
            self.target.clone(),
            self.remote_dir.clone(),
            self.multiplexing,
            self.opts.clone(),
        );
        Box::pin(async move {
            let conn = Arc::new(session::connect(&target, &opts).await.map_err(classify)?);
            let keepalive = Duration::from_secs(target.keepalive_secs.max(1) as u64);
            let mut notice = None;
            let mut stream = None;
            if mux {
                match bootstrap::ensure_remote(&conn, &dir, bootstrap::VERSION).await {
                    Ok(inst) => {
                        let ch = conn
                            .handle
                            .channel_open_session()
                            .await
                            .map_err(|e| DialFailure::Transient(e.to_string()))?;
                        let cmd = format!("{} proxy", bootstrap::sh_path(&inst.binary));
                        ch.exec(true, cmd.into_bytes())
                            .await
                            .map_err(|e| DialFailure::Transient(e.to_string()))?;
                        stream = Some(Box::new(ch.into_stream()) as Box<dyn crate::link::Duplex>);
                    }
                    Err(BootstrapError::Ssh(e)) => return Err(classify(e)),
                    Err(e) => {
                        notice = Some(format!("Persistent sessions are off for this host: {e}"))
                    }
                }
            }
            let stream = stream.unwrap_or_else(|| crate::plain::stream(conn.clone()));
            Ok(Dialed {
                stream,
                keepalive,
                keep: Box::new(conn),
                notice,
            })
        })
    }
}
