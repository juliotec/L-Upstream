//! The keys the caches are found by: a hash of everything a cached thing was made from.
//!
//! A cache hit is right only if the key holds every input that changes the output, and fast only if it holds nothing
//! else. A [`Key`] is fed named parts in a fixed order, so two builds' keys differ exactly where their inputs do, and
//! [`fingerprint`] gives the short form the log prints for each part (two builds' logs side by side then say which part
//! changed when an update compiled everything again). SHA-1: the object store's keys are 40 hex digits.

use sha1_smol::Sha1;
use std::path::Path;

/// A key being made.
#[derive(Clone, Default)]
pub struct Key {
    h: Sha1,
}

impl Key {
    /// An empty key.
    pub fn new() -> Key {
        Key::default()
    }

    /// A named part: its name and its length go in too, so ("ab", "c") and ("a", "bc") make different keys.
    pub fn part(&mut self, name: &str, bytes: &[u8]) -> &mut Key {
        for chunk in [name.as_bytes(), &(bytes.len() as u64).to_le_bytes(), bytes] {
            self.h.update(chunk);
        }
        self
    }

    /// A named part from a string.
    pub fn text(&mut self, name: &str, text: &str) -> &mut Key {
        self.part(name, text.as_bytes())
    }

    /// A file's contents, named by its path relative to `base` (so a work folder elsewhere keeps its keys).
    pub fn file(&mut self, base: &Path, path: &Path) -> crate::Result<&mut Key> {
        use crate::error::IoContext;
        let bytes = std::fs::read(path).at(path)?;
        let rel = path.strip_prefix(base).unwrap_or(path).to_string_lossy().replace('\\', "/");
        Ok(self.part(&rel, &bytes))
    }

    /// The key, 40 hex digits.
    pub fn hex(&self) -> String {
        self.h.digest().to_string()
    }
}

/// The SHA-1 of `bytes`, 40 hex digits.
pub fn sha1_hex(bytes: &[u8]) -> String {
    Sha1::from(bytes).digest().to_string()
}

/// Bytes as lowercase hex digits (a SHA-256's 32 bytes as 64).
pub fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::with_capacity(2 * bytes.len()), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The SHA-1 of a file's bytes, streamed (a compiler or a game's exe is hundreds of MB), 40 hex digits.
pub fn file_sha1(path: &Path) -> crate::Result<String> {
    use crate::error::IoContext;
    use std::io::Read;
    let mut f = std::fs::File::open(path).at(path)?;
    let mut h = Sha1::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).at(path)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.digest().to_string())
}

/// A part's short fingerprint for the log: the first 10 hex digits of its SHA-1.
pub fn fingerprint(bytes: &[u8]) -> String {
    sha1_hex(bytes)[..10].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_are_kept_apart() {
        let a = Key::new().text("ab", "c").hex();
        let b = Key::new().text("a", "bc").hex();
        assert_ne!(a, b);
        assert_eq!(Key::new().text("ab", "c").hex(), a);
        assert_eq!(a.len(), 40);
    }

    #[test]
    fn sha1_as_python_s() {
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(fingerprint(b"abc"), "a9993e3647");
    }
}
