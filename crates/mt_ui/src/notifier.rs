//! OS desktop notifications for `ctl pane notify` (SPEC §12.4).
//!
//! Delivery sits behind [`Notifier`] so tests (and headless runs) use a fake. The system
//! implementation shows the notification on a short-lived thread because D-Bus and the macOS
//! notification centre can block. Failures are logged and otherwise ignored: a missing
//! notification daemon must never break `ctl`. On macOS an unbundled dev binary may show
//! nothing; a bundled `.app` is reliable.

use std::sync::Arc;

pub trait Notifier: Send + Sync {
    fn notify(&self, summary: &str, body: &str);
}

/// Shows notifications through the platform's notification service.
pub struct SystemNotifier;

impl Notifier for SystemNotifier {
    fn notify(&self, summary: &str, body: &str) {
        let (summary, body) = (summary.to_string(), body.to_string());
        std::thread::spawn(move || {
            let result = notify_rust::Notification::new()
                .appname("microterm")
                .summary(&summary)
                .body(&body)
                .show();
            if let Err(e) = result {
                eprintln!("microterm: desktop notification failed: {e}");
            }
        });
    }
}

pub fn system() -> Arc<dyn Notifier> {
    Arc::new(SystemNotifier)
}

#[cfg(test)]
pub use fake::FakeNotifier;

#[cfg(test)]
mod fake {
    use std::sync::Mutex;

    use super::Notifier;

    /// Records what would have been shown.
    #[derive(Default)]
    pub struct FakeNotifier {
        pub sent: Mutex<Vec<(String, String)>>,
    }

    impl Notifier for FakeNotifier {
        fn notify(&self, summary: &str, body: &str) {
            self.sent
                .lock()
                .unwrap()
                .push((summary.to_string(), body.to_string()));
        }
    }
}
