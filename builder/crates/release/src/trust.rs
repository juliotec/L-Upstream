//! The release key and a release's signed `SHA256SUMS` (docs/dev/reference/releases.md, "Signing"; the Longshot
//! Launcher's Core/ReleaseTrust.cs, ported).
//!
//! CI signs `SHA256SUMS` (`<64 hex>  <name>` lines, sha256sum's format) with the release key, ECDSA P-256:
//! `SHA256SUMS.sig` is the base64 of the DER signature of its exact bytes. One signature by a trusted key is enough;
//! the names it gives are then each file's SHA-256, which every download is checked against. The keys trusted are
//! scripts/release_keys.json's (built in), and one more from a PEM file named by `LONGSHOT_LAUNCHER_TRUST` (or
//! `LONGSHOT_TRUST`), for trying a release signed with a test key: never set for players.

use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use std::collections::BTreeMap;

/// The largest `SHA256SUMS` read.
pub const MAX_SUMS: u64 = 1 << 20;
/// The largest `SHA256SUMS.sig` read.
pub const MAX_SIG: u64 = 4096;
/// The variables naming one more trusted key's PEM file (the launcher's, then the builder's own).
pub const TRUST_VARIABLES: [&str; 2] = ["LONGSHOT_LAUNCHER_TRUST", "LONGSHOT_TRUST"];

/// scripts/release_keys.json, as the builder is built with it.
const BUILT_IN: &str = include_str!("../../../../scripts/release_keys.json");

/// The DER prefix of a P-256 public key's SubjectPublicKeyInfo: the key's uncompressed point (65 bytes) follows.
const SPKI_P256: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce,
    0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

/// A key releases are taken from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key(VerifyingKey);

impl Key {
    /// The key of a SubjectPublicKeyInfo (DER), ECDSA P-256 alone.
    pub fn from_spki(der: &[u8]) -> Result<Key, String> {
        let point =
            der.strip_prefix(&SPKI_P256[..]).filter(|p| p.len() == 65).ok_or("not an ECDSA P-256 public key")?;
        VerifyingKey::from_sec1_bytes(point).map(Key).map_err(|_| "not a point of P-256".into())
    }

    /// The key of a SubjectPublicKeyInfo in base64 (release_keys.json's).
    pub fn from_base64(text: &str) -> Result<Key, String> {
        Key::from_spki(&base64(text)?)
    }

    /// The key of a PEM public key (`-----BEGIN PUBLIC KEY-----`, openssl's).
    pub fn from_pem(text: &str) -> Result<Key, String> {
        let body: String = text.lines().filter(|l| !l.trim_start().starts_with("-----")).collect();
        Key::from_base64(&body)
    }

    /// The key of a verifying key (the tests' own).
    #[must_use]
    pub fn of(key: VerifyingKey) -> Key {
        Key(key)
    }
}

/// The keys this builder trusts: release_keys.json's, and `TRUST_VARIABLES`' PEM file's when one is named and reads
/// (`get`: the environment).
#[must_use]
pub fn keys(get: &dyn Fn(&str) -> Option<String>) -> Vec<Key> {
    let mut keys: Vec<Key> = serde_json::from_str::<serde_json::Value>(BUILT_IN)
        .ok()
        .and_then(|v| v.get("keys")?.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|k| Key::from_base64(k.as_str()?).ok())
        .collect();
    for var in TRUST_VARIABLES {
        if let Some(path) = get(var).filter(|p| !p.trim().is_empty())
            && let Ok(k) = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| Key::from_pem(&t))
        {
            keys.push(k);
        }
    }
    keys
}

/// Standard base64, padded or not, white space ignored.
pub fn base64(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes().filter(|c| !c.is_ascii_whitespace()) {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return Err("not base64".into()),
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

/// A DER ECDSA signature (`SEQUENCE { INTEGER r, INTEGER s }`) as r and s, 32 bytes each.
fn der_signature(der: &[u8]) -> Option<Signature> {
    let body = match der {
        [0x30, len, rest @ ..] if usize::from(*len) == rest.len() && *len < 0x80 => rest,
        _ => return None,
    };
    let int = |b: &[u8]| -> Option<([u8; 32], usize)> {
        let [0x02, len, rest @ ..] = b else { return None };
        let len = usize::from(*len);
        let mut v = rest.get(..len)?;
        while v.len() > 1 && v[0] == 0 {
            v = &v[1..];
        }
        if v.len() > 32 {
            return None;
        }
        let mut n = [0u8; 32];
        n[32 - v.len()..].copy_from_slice(v);
        Some((n, 2 + len))
    };
    let (r, used) = int(body)?;
    let (s, used2) = int(&body[used..])?;
    if used + used2 != body.len() {
        return None;
    }
    let mut rs = [0u8; 64];
    rs[..32].copy_from_slice(&r);
    rs[32..].copy_from_slice(&s);
    Signature::from_slice(&rs).ok()
}

/// `SHA256SUMS`'s files and their SHA-256s, once a key of `keys` is found to have signed `sums` (`sig`: the base64
/// DER signature). The reason it is refused otherwise.
pub fn open(sums: &[u8], sig: &str, keys: &[Key]) -> Result<BTreeMap<String, String>, String> {
    if sums.len() as u64 > MAX_SUMS || sig.len() as u64 > MAX_SIG {
        return Err("its SHA256SUMS is too large".into());
    }
    if keys.is_empty() {
        return Err("this builder trusts no release key".into());
    }
    let signature =
        base64(sig.trim()).ok().and_then(|d| der_signature(&d)).ok_or("its SHA256SUMS.sig can't be read")?;
    if !keys.iter().any(|k| k.0.verify(sums, &signature).is_ok()) {
        return Err("its SHA256SUMS is not signed by a key this builder trusts".into());
    }
    let text = std::str::from_utf8(sums).map_err(|_| "its SHA256SUMS is not text")?;
    let mut files = BTreeMap::new();
    for line in text.split('\n').map(|l| l.trim_end_matches('\r')).filter(|l| !l.is_empty()) {
        let bad = || format!("not a SHA256SUMS line: {line}");
        let (sha, rest) = line.split_at_checked(64).ok_or_else(bad)?;
        let name = rest.strip_prefix(' ').and_then(|r| r.strip_prefix([' ', '*'])).ok_or_else(bad)?;
        if !sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || name.is_empty()
            || name.contains(['/', '\\'])
        {
            return Err(bad());
        }
        if files.insert(name.to_string(), sha.to_string()).is_some() {
            return Err("its SHA256SUMS names a file twice".into());
        }
    }
    Ok(files)
}

/// Keys and signatures for tests, this crate's and (feature `testkit`) other crates'.
#[cfg(any(test, feature = "testkit"))]
pub mod tests {
    use super::*;
    pub use p256::ecdsa::SigningKey;
    use p256::ecdsa::signature::Signer;

    /// The tests' signing key (a fixed scalar), and its public key as a trusted [`Key`].
    pub fn test_key() -> (SigningKey, Key) {
        let sk = SigningKey::from_slice(&[7u8; 32]).unwrap();
        let k = Key::of(*sk.verifying_key());
        (sk, k)
    }

    /// Another signing key (a scalar of `seed`, not 0), and its public key.
    pub fn key(seed: u8) -> (SigningKey, Key) {
        let sk = SigningKey::from_slice(&[seed.max(1); 32]).unwrap();
        let k = Key::of(*sk.verifying_key());
        (sk, k)
    }

    /// `sums` signed by `sk`, as SHA256SUMS.sig is (base64 of DER).
    pub fn sign(sk: &SigningKey, sums: &[u8]) -> String {
        let sig: Signature = sk.sign(sums);
        let (r, s) = sig.split_bytes();
        let int = |b: &[u8]| {
            let mut v: Vec<u8> = b.iter().copied().skip_while(|&x| x == 0).collect();
            if v.first().is_none_or(|&x| x >= 0x80) {
                v.insert(0, 0);
            }
            let mut out = vec![0x02, v.len() as u8];
            out.extend(v);
            out
        };
        let body = [int(&r), int(&s)].concat();
        let mut der = vec![0x30, body.len() as u8];
        der.extend(body);
        b64(&der)
    }

    /// `b` in base64 (with padding).
    pub fn b64(b: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in b.chunks(3) {
            let n = (u32::from(c[0]) << 16)
                | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
                | u32::from(*c.get(2).unwrap_or(&0));
            for i in 0..4 {
                if i <= c.len() {
                    s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    s.push('=');
                }
            }
        }
        s
    }

    #[test]
    fn the_built_in_key_reads() {
        assert_eq!(keys(&|_| None).len(), 1, "release_keys.json's one key");
    }

    #[test]
    fn a_signed_list_opens_and_any_other_is_refused() {
        let (sk, k) = test_key();
        let sums = format!("{}  a.json\n{} *b\n", "a".repeat(64), "0".repeat(64));
        let sig = sign(&sk, sums.as_bytes());
        let files = open(sums.as_bytes(), &sig, std::slice::from_ref(&k)).unwrap();
        assert_eq!(files.get("a.json").map(String::as_str), Some("a".repeat(64).as_str()));
        assert_eq!(files.len(), 2);
        // (another key's, changed bytes, no key, a bad line)
        assert!(open(sums.as_bytes(), &sig, &keys(&|_| None)).unwrap_err().contains("not signed"));
        let changed = sums.replace("a.json", "c.json");
        assert!(open(changed.as_bytes(), &sig, std::slice::from_ref(&k)).is_err());
        assert!(open(sums.as_bytes(), &sig, &[]).unwrap_err().contains("no release key"));
        let bad = "xyz  a\n";
        assert!(
            open(bad.as_bytes(), &sign(&sk, bad.as_bytes()), std::slice::from_ref(&k)).unwrap_err().contains("line")
        );
        let twice = format!("{0}  a\n{0}  a\n", "1".repeat(64));
        assert!(open(twice.as_bytes(), &sign(&sk, twice.as_bytes()), &[k]).unwrap_err().contains("twice"));
    }

    #[test]
    fn a_pem_key_is_trusted_through_the_variable() {
        let (_, k) = test_key();
        let point = k.0.to_encoded_point(false);
        let mut spki = SPKI_P256.to_vec();
        spki.extend_from_slice(point.as_bytes());
        let pem = format!("-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n", b64(&spki));
        assert_eq!(Key::from_pem(&pem).unwrap(), k);
        assert!(Key::from_pem("-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----").is_err());
    }
}
