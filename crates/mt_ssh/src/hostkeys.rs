//! Host key verification against `known_hosts` (SPEC §11.2): plain and hashed entries; an
//! unknown key needs the user's confirmation; a changed key is a hard failure.

use std::path::{Path, PathBuf};

use russh::keys::{known_hosts, Error as KeyError, HashAlg, PublicKey};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyStatus {
    Known,
    Unknown,
    /// A different key of the same algorithm is recorded at `line`.
    Changed {
        line: usize,
    },
}

pub fn default_known_hosts() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".ssh/known_hosts"))
        .unwrap_or_else(|| PathBuf::from(".ssh/known_hosts"))
}

pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

pub fn check(
    path: &Path,
    host: &str,
    port: u16,
    key: &PublicKey,
) -> Result<HostKeyStatus, KeyError> {
    if !path.exists() {
        return Ok(HostKeyStatus::Unknown);
    }
    match known_hosts::check_known_hosts_path(host, port, key, path) {
        Ok(true) => Ok(HostKeyStatus::Known),
        Ok(false) => Ok(HostKeyStatus::Unknown),
        Err(KeyError::KeyChanged { line }) => Ok(HostKeyStatus::Changed { line }),
        Err(e) => Err(e),
    }
}

/// Appends the key to `known_hosts` (after the user chose *Trust and connect*).
pub fn trust(path: &Path, host: &str, port: u16, key: &PublicKey) -> Result<(), KeyError> {
    known_hosts::learn_known_hosts_path(host, port, key, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
    use russh::keys::ssh_key::PrivateKey;

    fn key(seed: u8) -> PublicKey {
        let kp = Ed25519Keypair::from_seed(&[seed; 32]);
        PrivateKey::new(KeypairData::Ed25519(kp), "")
            .unwrap()
            .public_key()
            .clone()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mtssh-hk-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("known_hosts")
    }

    #[test]
    fn unknown_then_trusted_then_changed() {
        let path = tmp("flow");
        let (a, b) = (key(1), key(2));
        assert_eq!(
            check(&path, "h.example", 22, &a).unwrap(),
            HostKeyStatus::Unknown
        );
        trust(&path, "h.example", 22, &a).unwrap();
        assert_eq!(
            check(&path, "h.example", 22, &a).unwrap(),
            HostKeyStatus::Known
        );
        assert!(matches!(
            check(&path, "h.example", 22, &b).unwrap(),
            HostKeyStatus::Changed { .. }
        ));
        // Other hosts and ports are independent.
        assert_eq!(
            check(&path, "other", 22, &a).unwrap(),
            HostKeyStatus::Unknown
        );
        trust(&path, "h.example", 2200, &b).unwrap();
        assert_eq!(
            check(&path, "h.example", 2200, &b).unwrap(),
            HostKeyStatus::Known
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[h.example]:2200 "));
    }

    #[test]
    fn hashed_entries_are_recognised() {
        let path = tmp("hashed");
        let k = key(3);
        // `ssh-keygen -H` style entry for host "hashed.example": salt/hash computed by HMAC-SHA1.
        std::fs::write(&path, format!("{}\n", hashed_line("hashed.example", &k))).unwrap();
        assert_eq!(
            check(&path, "hashed.example", 22, &k).unwrap(),
            HostKeyStatus::Known
        );
        assert_eq!(
            check(&path, "nope.example", 22, &k).unwrap(),
            HostKeyStatus::Unknown
        );
    }

    fn hashed_line(host: &str, k: &PublicKey) -> String {
        // HMAC-SHA1 per OpenSSH `HashKnownHosts`, built from sha2-free primitives is not
        // available; use the system `ssh-keygen` equivalent by hand with a fixed salt.
        let salt = [7u8; 20];
        let mac = hmac_sha1(&salt, host.as_bytes());
        use std::fmt::Write;
        let b64 = |b: &[u8]| {
            const T: &[u8; 64] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut s = String::new();
            for c in b.chunks(3) {
                let n = (c[0] as u32) << 16
                    | (*c.get(1).unwrap_or(&0) as u32) << 8
                    | *c.get(2).unwrap_or(&0) as u32;
                for i in 0..4 {
                    if i <= c.len() {
                        s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
                    } else {
                        s.push('=');
                    }
                }
            }
            s
        };
        let mut out = String::new();
        write!(
            out,
            "|1|{}|{} {}",
            b64(&salt),
            b64(&mac),
            k.to_openssh().unwrap()
        )
        .unwrap();
        out
    }

    fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
        use sha1_shim::sha1;
        let mut k = [0u8; 64];
        k[..key.len()].copy_from_slice(key);
        let mut ipad: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
        ipad.extend_from_slice(msg);
        let inner = sha1(&ipad);
        let mut opad: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
        opad.extend_from_slice(&inner);
        sha1(&opad)
    }

    mod sha1_shim {
        pub fn sha1(data: &[u8]) -> [u8; 20] {
            let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
            let mut m = data.to_vec();
            m.push(0x80);
            while m.len() % 64 != 56 {
                m.push(0);
            }
            m.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
            for chunk in m.chunks(64) {
                let mut w = [0u32; 80];
                for i in 0..16 {
                    w[i] = u32::from_be_bytes([
                        chunk[4 * i],
                        chunk[4 * i + 1],
                        chunk[4 * i + 2],
                        chunk[4 * i + 3],
                    ]);
                }
                for i in 16..80 {
                    w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
                }
                let [mut a, mut b, mut c, mut d, mut e] = h;
                for (i, wi) in w.iter().enumerate() {
                    let (f, k) = match i {
                        0..=19 => ((b & c) | (!b & d), 0x5A827999),
                        20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                        40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                        _ => (b ^ c ^ d, 0xCA62C1D6u32),
                    };
                    let t = a
                        .rotate_left(5)
                        .wrapping_add(f)
                        .wrapping_add(e)
                        .wrapping_add(k)
                        .wrapping_add(*wi);
                    e = d;
                    d = c;
                    c = b.rotate_left(30);
                    b = a;
                    a = t;
                }
                for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
                    *x = x.wrapping_add(y);
                }
            }
            let mut out = [0u8; 20];
            for (i, x) in h.iter().enumerate() {
                out[4 * i..4 * i + 4].copy_from_slice(&x.to_be_bytes());
            }
            out
        }
    }
}
