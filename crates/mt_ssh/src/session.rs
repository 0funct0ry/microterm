//! Connecting and authenticating (SPEC §11.2): direct TCP, `ProxyJump` chains over
//! `direct-tcpip` channels, `ProxyCommand` stdio transports; host-key checks; auth order
//! agent → identity files → keyboard-interactive → password. Secrets are never logged.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use russh::client::{self, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::agent::client::AgentClient;
use russh::keys::agent::AgentIdentity;
use russh::keys::{Error as KeyError, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::config::{Proxy, Target};
use crate::hostkeys::{self, HostKeyStatus};
use crate::prompt::{HostKeyPrompt, Prompter, Secret, SecretPrompt};

#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("timed out connecting to {0}")]
    Timeout(String),
    #[error("the host key for {host} has changed (known_hosts line {line}, now {fingerprint}); refusing to connect")]
    HostKeyChanged {
        host: String,
        line: usize,
        fingerprint: String,
    },
    #[error("the host key for {0} was not trusted")]
    HostKeyRejected(String),
    #[error("authentication failed for {user}@{host}")]
    AuthFailed { user: String, host: String },
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

impl From<russh::Error> for SshError {
    fn from(e: russh::Error) -> Self {
        SshError::Connect(e.to_string())
    }
}

#[derive(Debug)]
enum HostKeyFailure {
    Changed { line: usize, fingerprint: String },
    Rejected,
}

/// Verifies the server key during the handshake.
pub struct Checker {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    prompter: Arc<dyn Prompter>,
    failure: Arc<Mutex<Option<HostKeyFailure>>>,
}

impl client::Handler for Checker {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key: PublicKey = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(c) => PublicKey::from(c.public_key().clone()),
        };
        let fingerprint = hostkeys::fingerprint(&key);
        let status = hostkeys::check(&self.known_hosts, &self.host, self.port, &key)
            .map_err(|_| russh::Error::CouldNotReadKey)?;
        match status {
            HostKeyStatus::Known => Ok(true),
            HostKeyStatus::Changed { line } => {
                *self.failure.lock().unwrap() = Some(HostKeyFailure::Changed { line, fingerprint });
                Ok(false)
            }
            HostKeyStatus::Unknown => {
                let ok = self
                    .prompter
                    .confirm_host_key(HostKeyPrompt {
                        host: self.host.clone(),
                        port: self.port,
                        fingerprint,
                        key_type: key.algorithm().to_string(),
                    })
                    .await;
                if ok {
                    hostkeys::trust(&self.known_hosts, &self.host, self.port, &key)
                        .map_err(|_| russh::Error::CouldNotReadKey)?;
                } else {
                    *self.failure.lock().unwrap() = Some(HostKeyFailure::Rejected);
                }
                Ok(ok)
            }
        }
    }
}

/// An established, authenticated connection. Earlier hops of a `ProxyJump` chain are kept
/// alive here because the later hops are tunnelled through them.
pub struct Connection {
    pub handle: Handle<Checker>,
    _hops: Vec<Handle<Checker>>,
}

impl Connection {
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    pub async fn disconnect(&self) {
        let _ = self
            .handle
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
    }

    /// Runs `command` and returns its stdout and exit status.
    pub async fn exec_capture(&self, command: &str) -> Result<(Vec<u8>, Option<u32>), SshError> {
        let mut ch = self.handle.channel_open_session().await?;
        ch.exec(true, command.as_bytes().to_vec()).await?;
        let (mut out, mut status) = (Vec::new(), None);
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } => out.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                _ => {}
            }
        }
        Ok((out, status))
    }
}

#[derive(Clone)]
pub struct ConnectOptions {
    pub known_hosts: PathBuf,
    pub timeout: Duration,
    pub prompter: Arc<dyn Prompter>,
}

/// stdio of a `ProxyCommand` process as a duplex stream.
struct CommandStream {
    _child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
}

impl AsyncRead for CommandStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

impl AsyncWrite for CommandStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stdin).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdin).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdin).poll_shutdown(cx)
    }
}

fn config_for(target: &Target) -> Arc<client::Config> {
    Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(target.keepalive_secs.max(1) as u64)),
        keepalive_max: 3,
        nodelay: true,
        ..client::Config::default()
    })
}

async fn handshake<S>(
    stream: S,
    target: &Target,
    opts: &ConnectOptions,
) -> Result<Handle<Checker>, SshError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let failure = Arc::new(Mutex::new(None));
    let checker = Checker {
        host: target.host.clone(),
        port: target.port,
        known_hosts: opts.known_hosts.clone(),
        prompter: opts.prompter.clone(),
        failure: failure.clone(),
    };
    let mut handle = match client::connect_stream(config_for(target), stream, checker).await {
        Ok(h) => h,
        Err(e) => {
            return Err(match failure.lock().unwrap().take() {
                Some(HostKeyFailure::Changed { line, fingerprint }) => SshError::HostKeyChanged {
                    host: target.host.clone(),
                    line,
                    fingerprint,
                },
                Some(HostKeyFailure::Rejected) => SshError::HostKeyRejected(target.host.clone()),
                None => SshError::Connect(e.to_string()),
            })
        }
    };
    authenticate(&mut handle, target, opts).await?;
    Ok(handle)
}

fn default_identities() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .iter()
        .map(|n| PathBuf::from(&home).join(".ssh").join(n))
        .collect()
}

async fn load_key(
    path: &Path,
    prompter: &dyn Prompter,
) -> Result<Option<russh::keys::PrivateKey>, SshError> {
    let pass = |p: Option<&str>| russh::keys::load_secret_key(path, p);
    match pass(None) {
        Ok(k) => Ok(Some(k)),
        Err(KeyError::KeyIsEncrypted) => {
            for _ in 0..3 {
                let Some(secret) = prompter
                    .secret(SecretPrompt::Passphrase {
                        path: path.display().to_string(),
                    })
                    .await
                else {
                    return Err(SshError::Cancelled);
                };
                if let Ok(k) = pass(Some(secret.expose())) {
                    return Ok(Some(k));
                }
            }
            Ok(None)
        }
        Err(_) => Ok(None),
    }
}

async fn authenticate(
    handle: &mut Handle<Checker>,
    target: &Target,
    opts: &ConnectOptions,
) -> Result<(), SshError> {
    let user = target.user.clone();
    let failed = || SshError::AuthFailed {
        user: target.user.clone(),
        host: target.host.clone(),
    };
    let hash = handle
        .best_supported_rsa_hash()
        .await
        .ok()
        .flatten()
        .flatten();

    // 1. ssh-agent.
    if !target.identities_only {
        if let Ok(mut agent) = AgentClient::connect_env().await {
            if let Ok(ids) = agent.request_identities().await {
                for id in ids {
                    let AgentIdentity::PublicKey { key, .. } = id else {
                        continue;
                    };
                    if let Ok(r) = handle
                        .authenticate_publickey_with(user.clone(), key, hash, &mut agent)
                        .await
                    {
                        if r.success() {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    // 2. Identity files.
    let mut files = target.identity_files.clone();
    if !target.identities_only {
        files.extend(default_identities());
    }
    let mut seen = Vec::new();
    for f in files {
        if seen.contains(&f) || !f.exists() {
            continue;
        }
        seen.push(f.clone());
        if let Some(key) = load_key(&f, opts.prompter.as_ref()).await? {
            let r = handle
                .authenticate_publickey(
                    user.clone(),
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                )
                .await?;
            if r.success() {
                return Ok(());
            }
        }
    }

    // 3. Keyboard-interactive.
    if let Ok(mut resp) = handle
        .authenticate_keyboard_interactive_start(user.clone(), None)
        .await
    {
        for _ in 0..6 {
            match resp {
                KeyboardInteractiveAuthResponse::Success => return Ok(()),
                KeyboardInteractiveAuthResponse::Failure { .. } => break,
                KeyboardInteractiveAuthResponse::InfoRequest { name, prompts, .. } => {
                    let mut answers = Vec::new();
                    for p in prompts {
                        let Some(s) = opts
                            .prompter
                            .secret(SecretPrompt::Interactive {
                                name: name.clone(),
                                text: p.prompt,
                                echo: p.echo,
                            })
                            .await
                        else {
                            return Err(SshError::Cancelled);
                        };
                        answers.push(s.expose().to_string());
                    }
                    resp = handle
                        .authenticate_keyboard_interactive_respond(answers)
                        .await?;
                }
            }
        }
    }

    // 4. Password.
    for _ in 0..3 {
        let Some(pw): Option<Secret> = opts
            .prompter
            .secret(SecretPrompt::Password {
                user: user.clone(),
                host: target.host.clone(),
            })
            .await
        else {
            return Err(SshError::Cancelled);
        };
        if handle
            .authenticate_password(user.clone(), pw.expose().to_string())
            .await?
            .success()
        {
            return Ok(());
        }
    }
    Err(failed())
}

async fn tcp(host: &str, port: u16, timeout: Duration) -> Result<tokio::net::TcpStream, SshError> {
    let label = format!("{host}:{port}");
    match tokio::time::timeout(timeout, tokio::net::TcpStream::connect((host, port))).await {
        Err(_) => Err(SshError::Timeout(label)),
        Ok(Err(e)) => Err(SshError::Connect(format!("{label}: {e}"))),
        Ok(Ok(s)) => {
            let _ = s.set_nodelay(true);
            Ok(s)
        }
    }
}

/// Opens a connection to `target`, following its proxy configuration.
pub async fn connect(target: &Target, opts: &ConnectOptions) -> Result<Connection, SshError> {
    tokio::time::timeout(opts.timeout * 4, connect_inner(target, opts))
        .await
        .map_err(|_| SshError::Timeout(target.host.clone()))?
}

fn connect_inner<'a>(
    target: &'a Target,
    opts: &'a ConnectOptions,
) -> Pin<Box<dyn std::future::Future<Output = Result<Connection, SshError>> + Send + 'a>> {
    Box::pin(async move {
        match &target.proxy {
            Proxy::None => {
                let s = tcp(&target.host, target.port, opts.timeout).await?;
                Ok(Connection {
                    handle: handshake(s, target, opts).await?,
                    _hops: Vec::new(),
                })
            }
            Proxy::Command(cmd) => {
                let mut child = tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(cmd)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .kill_on_drop(true)
                    .spawn()
                    .map_err(|e| SshError::Connect(format!("ProxyCommand: {e}")))?;
                let stream = CommandStream {
                    stdin: child.stdin.take().unwrap(),
                    stdout: child.stdout.take().unwrap(),
                    _child: child,
                };
                Ok(Connection {
                    handle: handshake(stream, target, opts).await?,
                    _hops: Vec::new(),
                })
            }
            Proxy::Jump(hops) => {
                // First hop directly, each next one through a direct-tcpip channel.
                let first = &hops[0];
                let mut direct = first.clone();
                direct.proxy = Proxy::None;
                let mut chain = vec![
                    handshake(
                        tcp(&direct.host, direct.port, opts.timeout).await?,
                        &direct,
                        opts,
                    )
                    .await?,
                ];
                for next in hops[1..].iter().chain(std::iter::once(target)) {
                    let via = chain.last().unwrap();
                    let ch = via
                        .channel_open_direct_tcpip(
                            next.host.clone(),
                            next.port as u32,
                            "127.0.0.1",
                            0,
                        )
                        .await?;
                    let mut leaf = next.clone();
                    leaf.proxy = Proxy::None;
                    let h = handshake(ch.into_stream(), &leaf, opts).await?;
                    chain.push(h);
                }
                let handle = chain.pop().unwrap();
                Ok(Connection {
                    handle,
                    _hops: chain,
                })
            }
        }
    })
}
