//! The background Tokio runtime that owns every SSH session (SPEC §3.4).

use std::io;
use std::sync::Arc;

use crate::link::{self, Dialer, LinkConfig, LinkEvent, LinkHandle};

pub struct SshRuntime {
    rt: tokio::runtime::Runtime,
}

impl SshRuntime {
    pub fn new() -> io::Result<Self> {
        Ok(SshRuntime {
            rt: tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("mt-ssh")
                .enable_all()
                .build()?,
        })
    }

    /// Starts a domain link on the runtime.
    pub fn start_link(
        &self,
        cfg: LinkConfig,
        dialer: Arc<dyn Dialer>,
    ) -> (LinkHandle, async_channel::Receiver<LinkEvent>) {
        link::start(self.rt.handle(), cfg, dialer)
    }
}
