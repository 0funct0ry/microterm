//! Interaction with the user during connect: host keys, passphrases, passwords (SPEC §11.2).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use zeroize::Zeroizing;

/// A secret that is wiped on drop and never printed.
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Secret(Zeroizing::new(s.into()))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(***)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyPrompt {
    pub host: String,
    pub port: u16,
    /// `SHA256:…`
    pub fingerprint: String,
    pub key_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretPrompt {
    /// Passphrase of an encrypted identity file.
    Passphrase {
        path: String,
    },
    Password {
        user: String,
        host: String,
    },
    /// One keyboard-interactive prompt; `echo` says whether typing may be shown.
    Interactive {
        name: String,
        text: String,
        echo: bool,
    },
}

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Implemented by the UI. Nothing returned here is stored or logged.
pub trait Prompter: Send + Sync {
    /// *Trust and connect* (`true`) or *Cancel*.
    fn confirm_host_key(&self, prompt: HostKeyPrompt) -> BoxFuture<bool>;
    /// `None` means the user cancelled.
    fn secret(&self, prompt: SecretPrompt) -> BoxFuture<Option<Secret>>;
}

/// Answers nothing: connections that need interaction fail instead of hanging (tests, CLI).
pub struct NoPrompts;

impl Prompter for NoPrompts {
    fn confirm_host_key(&self, _: HostKeyPrompt) -> BoxFuture<bool> {
        Box::pin(async { false })
    }
    fn secret(&self, _: SecretPrompt) -> BoxFuture<Option<Secret>> {
        Box::pin(async { None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_reach_debug_output() {
        let s = Secret::new("hunter2");
        assert_eq!(s.expose(), "hunter2");
        let shown = format!(
            "{s:?} {:?}",
            SecretPrompt::Password {
                user: "u".into(),
                host: "h".into()
            }
        );
        assert!(!shown.contains("hunter2"));
        assert!(format!("{:?}", Some(&s)).contains("***"));
    }
}

// ---- UI bridge ------------------------------------------------------------------------------

pub enum PromptKind {
    HostKey(HostKeyPrompt),
    Secret(SecretPrompt),
}

enum Answer {
    Confirm(bool),
    Secret(Option<Secret>),
}

/// A question for the user. Answer it with [`PromptRequest::confirm`] or
/// [`PromptRequest::secret`]; dropping it unanswered cancels (host key: refuse, secret: `None`).
pub struct PromptRequest {
    /// The SSH domain asking.
    pub domain: String,
    pub kind: PromptKind,
    reply: Option<tokio::sync::oneshot::Sender<Answer>>,
}

impl PromptRequest {
    pub fn confirm(mut self, yes: bool) {
        if let Some(r) = self.reply.take() {
            let _ = r.send(Answer::Confirm(yes));
        }
    }

    pub fn secret(mut self, s: Option<Secret>) {
        if let Some(r) = self.reply.take() {
            let _ = r.send(Answer::Secret(s));
        }
    }
}

/// The other end of [`PromptRequest::for_test`], for UI tests.
pub struct TestAnswer(tokio::sync::oneshot::Receiver<Answer>);

#[derive(Debug, PartialEq, Eq)]
pub enum Answered {
    Pending,
    Cancelled,
    Confirm(bool),
    Secret(Option<String>),
}

impl TestAnswer {
    pub fn poll(&mut self) -> Answered {
        use tokio::sync::oneshot::error::TryRecvError;
        match self.0.try_recv() {
            Ok(Answer::Confirm(b)) => Answered::Confirm(b),
            Ok(Answer::Secret(s)) => Answered::Secret(s.map(|s| s.expose().to_string())),
            Err(TryRecvError::Empty) => Answered::Pending,
            Err(TryRecvError::Closed) => Answered::Cancelled,
        }
    }
}

impl PromptRequest {
    /// A request and a handle to see how it was answered (tests).
    pub fn for_test(domain: &str, kind: PromptKind) -> (PromptRequest, TestAnswer) {
        let (reply, rx) = tokio::sync::oneshot::channel();
        (
            PromptRequest {
                domain: domain.into(),
                kind,
                reply: Some(reply),
            },
            TestAnswer(rx),
        )
    }
}

/// A [`Prompter`] that forwards every question to the UI over a channel.
pub struct ChannelPrompter {
    pub domain: String,
    pub tx: async_channel::Sender<PromptRequest>,
}

impl ChannelPrompter {
    async fn ask(&self, kind: PromptKind) -> Option<Answer> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(PromptRequest {
                domain: self.domain.clone(),
                kind,
                reply: Some(reply),
            })
            .await
            .ok()?;
        rx.await.ok()
    }
}

impl Prompter for Arc<ChannelPrompter> {
    fn confirm_host_key(&self, prompt: HostKeyPrompt) -> BoxFuture<bool> {
        let me = self.clone();
        Box::pin(async move {
            matches!(
                me.ask(PromptKind::HostKey(prompt)).await,
                Some(Answer::Confirm(true))
            )
        })
    }

    fn secret(&self, prompt: SecretPrompt) -> BoxFuture<Option<Secret>> {
        let me = self.clone();
        Box::pin(async move {
            match me.ask(PromptKind::Secret(prompt)).await {
                Some(Answer::Secret(s)) => s,
                _ => None,
            }
        })
    }
}

#[cfg(test)]
mod bridge_tests {
    use super::*;

    #[tokio::test]
    async fn answers_flow_back_and_dropping_cancels() {
        let (tx, rx) = async_channel::unbounded();
        let p: Arc<dyn Prompter> = Arc::new(Arc::new(ChannelPrompter {
            domain: "prod".into(),
            tx,
        }));
        let hk = HostKeyPrompt {
            host: "h".into(),
            port: 22,
            fingerprint: "SHA256:x".into(),
            key_type: "ssh-ed25519".into(),
        };

        let p2 = p.clone();
        let hk2 = hk.clone();
        let t = tokio::spawn(async move { p2.confirm_host_key(hk2).await });
        let req = rx.recv().await.unwrap();
        assert_eq!(req.domain, "prod");
        req.confirm(true);
        assert!(t.await.unwrap());

        let p2 = p.clone();
        let t = tokio::spawn(async move { p2.confirm_host_key(hk).await });
        drop(rx.recv().await.unwrap()); // unanswered: refused
        assert!(!t.await.unwrap());

        let p2 = p.clone();
        let t = tokio::spawn(async move {
            p2.secret(SecretPrompt::Password {
                user: "u".into(),
                host: "h".into(),
            })
            .await
        });
        rx.recv().await.unwrap().secret(Some(Secret::new("pw")));
        assert_eq!(t.await.unwrap().unwrap().expose(), "pw");
    }
}
