//! Remote daemon bootstrap (SPEC §11.3): find or install `microterm-mux-<version>` on the
//! host, then hand back the command that runs its `proxy`.
//!
//! The binary comes from the local cache `~/.cache/microterm/mux/<version>/<target>/`, filled on
//! first use from the matching GitHub release and verified against its `SHA256SUMS`.
//! `MICROTERM_MUX_BINARY` points at a local build instead (development, tests).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::session::{Connection, SshError};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES: &str = "https://github.com/0funct0ry/microterm/releases/download";

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("unsupported remote platform `{0}`")]
    UnsupportedPlatform(String),
    #[error("could not obtain microterm-mux for {target}: {reason}")]
    Fetch { target: String, reason: String },
    #[error("checksum mismatch for {file}: expected {expected}, got {actual}")]
    Checksum {
        file: String,
        expected: String,
        actual: String,
    },
    #[error("could not install on the remote host: {0}")]
    Install(String),
    #[error(transparent)]
    Ssh(#[from] SshError),
}

/// `uname -sm` output → release target triple.
pub fn target_for_uname(out: &str) -> Option<&'static str> {
    let mut it = out.split_whitespace();
    let (os, arch) = (it.next()?, it.next()?);
    match (os, arch) {
        ("Linux", "x86_64") | ("Linux", "amd64") => Some("x86_64-unknown-linux-musl"),
        ("Linux", "aarch64") | ("Linux", "arm64") => Some("aarch64-unknown-linux-musl"),
        ("Darwin", "arm64") | ("Darwin", "aarch64") => Some("aarch64-apple-darwin"),
        ("Darwin", "x86_64") => Some("x86_64-apple-darwin"),
        _ => None,
    }
}

/// `<remote_dir>/bin/microterm-mux-<version>` (with a leading `~` left for the remote shell).
pub fn remote_binary(remote_dir: &str, version: &str) -> String {
    format!(
        "{}/bin/microterm-mux-{version}",
        remote_dir.trim_end_matches('/')
    )
}

/// Single-quotes a path for `sh`, keeping a leading `~/` expandable.
pub fn sh_path(p: &str) -> String {
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    match p.strip_prefix("~/") {
        Some(rest) => format!("\"$HOME\"/{}", quote(rest)),
        None => quote(p),
    }
}

pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from(".cache"));
    base.join("microterm/mux")
}

pub fn cached_binary(version: &str, target: &str) -> PathBuf {
    cache_dir().join(version).join(target).join("microterm-mux")
}

pub fn release_url(version: &str, file: &str) -> String {
    format!("{RELEASES}/v{version}/{file}")
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The digest for `file` in a `SHA256SUMS` document (`<hex>  <name>` or `<hex> *<name>`).
pub fn parse_sha256sums(text: &str, file: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (hash, name) = l.split_once(char::is_whitespace)?;
        let name = name.trim().trim_start_matches('*');
        (name == file && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

pub fn verify(file: &str, data: &[u8], sums: &str) -> Result<(), BootstrapError> {
    let expected = parse_sha256sums(sums, file).ok_or_else(|| BootstrapError::Fetch {
        target: file.into(),
        reason: "not listed in SHA256SUMS".into(),
    })?;
    let actual = sha256_hex(data);
    if expected == actual {
        Ok(())
    } else {
        Err(BootstrapError::Checksum {
            file: file.into(),
            expected,
            actual,
        })
    }
}

fn http_get(url: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let resp = ureq::get(url).call().map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    resp.into_body()
        .into_reader()
        .take(256 * 1024 * 1024)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

/// The local file to upload for `target`: the dev override, the cache, or a verified download.
pub fn obtain_binary(version: &str, target: &str) -> Result<PathBuf, BootstrapError> {
    if let Some(p) = std::env::var_os("MICROTERM_MUX_BINARY").map(PathBuf::from) {
        return if p.is_file() {
            Ok(p)
        } else {
            Err(BootstrapError::Fetch {
                target: target.into(),
                reason: format!("MICROTERM_MUX_BINARY={} is not a file", p.display()),
            })
        };
    }
    let cached = cached_binary(version, target);
    if cached.is_file() {
        return Ok(cached);
    }
    let fetch_err = |reason: String| BootstrapError::Fetch {
        target: target.into(),
        reason,
    };
    let asset = format!("microterm-mux-{target}");
    let sums = http_get(&release_url(version, "SHA256SUMS")).map_err(fetch_err)?;
    let data = http_get(&release_url(version, &asset)).map_err(fetch_err)?;
    verify(&asset, &data, &String::from_utf8_lossy(&sums))?;
    let dir = cached.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|e| fetch_err(e.to_string()))?;
    let tmp = dir.join(format!(".download-{}", std::process::id()));
    std::fs::write(&tmp, &data).map_err(|e| fetch_err(e.to_string()))?;
    std::fs::rename(&tmp, &cached).map_err(|e| fetch_err(e.to_string()))?;
    Ok(cached)
}

/// What the bootstrap ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// Remote path of the binary (`~`-relative paths stay unexpanded).
    pub binary: String,
    /// `true` when it had to be uploaded.
    pub uploaded: bool,
}

async fn upload(conn: &Connection, local: &Path, dest_tmp: &str) -> Result<(), BootstrapError> {
    let data = tokio::fs::read(local)
        .await
        .map_err(|e| BootstrapError::Install(format!("{}: {e}", local.display())))?;
    // SFTP first (SPEC §11.3); plain `cat >` over an exec channel when SFTP is unavailable.
    let via_sftp = async {
        let ch = conn.handle.channel_open_session().await.ok()?;
        ch.request_subsystem(true, "sftp").await.ok()?;
        let sftp = russh_sftp::client::SftpSession::new(ch.into_stream())
            .await
            .ok()?;
        let home = sftp.canonicalize(".").await.ok()?;
        let path = match dest_tmp.strip_prefix("~/") {
            Some(rest) => format!("{}/{rest}", home.trim_end_matches('/')),
            None => dest_tmp.to_string(),
        };
        let mut f = sftp.create(path).await.ok()?;
        f.write_all(&data).await.ok()?;
        f.shutdown().await.ok()?;
        Some(())
    };
    if via_sftp.await.is_some() {
        return Ok(());
    }
    let mut ch = conn
        .handle
        .channel_open_session()
        .await
        .map_err(|e| BootstrapError::Install(e.to_string()))?;
    ch.exec(true, format!("cat > {}", sh_path(dest_tmp)).into_bytes())
        .await
        .map_err(|e| BootstrapError::Install(e.to_string()))?;
    ch.data(&data[..])
        .await
        .map_err(|e| BootstrapError::Install(e.to_string()))?;
    ch.eof()
        .await
        .map_err(|e| BootstrapError::Install(e.to_string()))?;
    let mut status = None;
    while let Some(m) = ch.wait().await {
        if let russh::ChannelMsg::ExitStatus { exit_status } = m {
            status = Some(exit_status);
        }
    }
    if status == Some(0) {
        Ok(())
    } else {
        Err(BootstrapError::Install("upload failed".into()))
    }
}

/// Does the installed binary differ from `local`? Unknown (no hashing tool) counts as same.
async fn remote_differs(conn: &Connection, quoted_remote: &str, local: &Path) -> bool {
    let Ok(data) = tokio::fs::read(local).await else {
        return false;
    };
    let cmd = format!(
        "(sha256sum {quoted_remote} 2>/dev/null || shasum -a 256 {quoted_remote} 2>/dev/null)"
    );
    match conn.exec_capture(&cmd).await {
        Ok((out, Some(0))) => {
            let text = String::from_utf8_lossy(&out);
            text.split_whitespace()
                .next()
                .is_some_and(|h| !h.eq_ignore_ascii_case(&sha256_hex(&data)))
        }
        _ => false,
    }
}

/// Makes sure `<remote_dir>/bin/microterm-mux-<version>` exists on the host.
pub async fn ensure_remote(
    conn: &Connection,
    remote_dir: &str,
    version: &str,
) -> Result<Installed, BootstrapError> {
    let binary = remote_binary(remote_dir, version);
    let q = sh_path(&binary);
    let mut replaced_stale = false;
    let (out, status) = conn.exec_capture(&format!("{q} --version")).await?;
    if status == Some(0) && String::from_utf8_lossy(&out).contains(version) {
        // A development override keeps the version number while the code changes, so compare
        // the file itself and re-upload a stale one.
        let stale = match std::env::var_os("MICROTERM_MUX_BINARY").map(PathBuf::from) {
            Some(local) => remote_differs(conn, &q, &local).await,
            None => false,
        };
        if !stale {
            return Ok(Installed {
                binary,
                uploaded: false,
            });
        }
        replaced_stale = true;
    }
    let (out, _) = conn.exec_capture("uname -sm").await?;
    let uname = String::from_utf8_lossy(&out).trim().to_string();
    let target = target_for_uname(&uname).ok_or(BootstrapError::UnsupportedPlatform(uname))?;
    let (v, t) = (version.to_string(), target.to_string());
    let local = tokio::task::spawn_blocking(move || obtain_binary(&v, &t))
        .await
        .map_err(|e| BootstrapError::Install(e.to_string()))??;

    let tmp = format!("{binary}.tmp{}", std::process::id());
    let dir = format!("{}/bin", remote_dir.trim_end_matches('/'));
    let (_, st) = conn
        .exec_capture(&format!("mkdir -p {}", sh_path(&dir)))
        .await?;
    if st != Some(0) {
        return Err(BootstrapError::Install(format!("cannot create {dir}")));
    }
    upload(conn, &local, &tmp).await?;
    let (_, st) = conn
        .exec_capture(&format!(
            "chmod 755 {t} && mv -f {t} {q}",
            t = sh_path(&tmp)
        ))
        .await?;
    if st != Some(0) {
        return Err(BootstrapError::Install("chmod/mv failed".into()));
    }
    // Prove it runs here (wrong libc, noexec mount …) before relying on it.
    let (out, st) = conn.exec_capture(&format!("{q} --version")).await?;
    if st != Some(0) || !String::from_utf8_lossy(&out).contains(version) {
        return Err(BootstrapError::Install(
            "the uploaded binary does not run on this host".into(),
        ));
    }
    if replaced_stale {
        // The running daemon still executes the old code. Development only: this ends its panes.
        let _ = conn
            .exec_capture(
                "d=${XDG_RUNTIME_DIR:+$XDG_RUNTIME_DIR/microterm-mux}; d=${d:-/tmp/microterm-mux-$(id -u)}; kill $(cat \"$d/lock\") 2>/dev/null; true",
            )
            .await;
    }
    Ok(Installed {
        binary,
        uploaded: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uname_maps_to_release_targets() {
        assert_eq!(
            target_for_uname("Linux x86_64\n"),
            Some("x86_64-unknown-linux-musl")
        );
        assert_eq!(
            target_for_uname("Linux aarch64"),
            Some("aarch64-unknown-linux-musl")
        );
        assert_eq!(
            target_for_uname("Darwin arm64"),
            Some("aarch64-apple-darwin")
        );
        assert_eq!(
            target_for_uname("Darwin x86_64"),
            Some("x86_64-apple-darwin")
        );
        assert_eq!(target_for_uname("FreeBSD amd64"), None);
        assert_eq!(target_for_uname(""), None);
    }

    #[test]
    fn remote_paths_and_quoting() {
        assert_eq!(
            remote_binary("~/.local/share/microterm/", "1.0.0"),
            "~/.local/share/microterm/bin/microterm-mux-1.0.0"
        );
        assert_eq!(sh_path("~/a b/it's"), "\"$HOME\"/'a b/it'\\''s'");
        assert_eq!(sh_path("/opt/x"), "'/opt/x'");
    }

    #[test]
    fn checksums_are_parsed_and_verified() {
        let data = b"binary";
        let h = sha256_hex(data);
        let sums = format!("{h}  microterm-mux-x\n{} *other\n", "0".repeat(64));
        assert_eq!(
            parse_sha256sums(&sums, "microterm-mux-x").as_deref(),
            Some(h.as_str())
        );
        assert!(verify("microterm-mux-x", data, &sums).is_ok());
        assert!(matches!(
            verify("other", data, &sums),
            Err(BootstrapError::Checksum { .. })
        ));
        assert!(verify("missing", data, &sums).is_err());
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn release_urls_and_cache_layout() {
        assert_eq!(
            release_url("1.0.0", "SHA256SUMS"),
            "https://github.com/0funct0ry/microterm/releases/download/v1.0.0/SHA256SUMS"
        );
        let p = cached_binary("1.0.0", "aarch64-apple-darwin");
        assert!(p.ends_with("microterm/mux/1.0.0/aarch64-apple-darwin/microterm-mux"));
    }

    #[test]
    fn dev_override_must_be_a_file() {
        std::env::set_var("MICROTERM_MUX_BINARY", "/definitely/not/here");
        assert!(matches!(
            obtain_binary("0.0.0", "x"),
            Err(BootstrapError::Fetch { .. })
        ));
        std::env::set_var("MICROTERM_MUX_BINARY", std::env::current_exe().unwrap());
        assert!(obtain_binary("0.0.0", "x").is_ok());
        std::env::remove_var("MICROTERM_MUX_BINARY");
    }
}
