//! The finish stage's cache: each output kept while exactly what it is made from stays the same.
//!
//! Every output (a language's texts, a face's textures, the font, the shader list) has a [`Key`] of what it is made
//! from: the files it reads, by their contents, the options that change it, and its recipe's version (a constant
//! beside the code that makes it, raised when that code would make something else of the same inputs). After it is
//! made, a stamp records the key and the outputs' contents; the next build finds the key the same and the outputs as
//! they were, and keeps them. So one changed text archive remakes that language alone, and one face's hand-drawn
//! picture that face's textures alone.
//!
//! Files are hashed by their contents (SHA-1), and the hashes kept by path, size and modification time, so a file
//! that has not been touched is not read again: the shader list's inputs are 280 MB of the romfs.

use build_core::key::Key;
use build_core::{IoContext, Result, fsx};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

/// The memo of file hashes, in the cache's folder.
const MEMO: &str = "hashes.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Seen {
    size: u64,
    mtime: u128,
    sha1: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Memo {
    files: BTreeMap<String, Seen>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Stamp {
    key: String,
    outputs: Vec<(String, String)>,
}

/// The cache of a work folder's finish stage.
#[derive(Debug)]
pub struct Cache {
    dir: PathBuf,
    memo: Mutex<Memo>,
}

fn stat(path: &Path) -> Result<(u64, u128)> {
    let m = std::fs::metadata(path).at(path)?;
    let mtime = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    Ok((m.len(), mtime))
}

/// The SHA-1 of a file's contents, read a piece at a time.
pub fn sha1_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path).at(path)?;
    let mut h = sha1_smol::Sha1::new();
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

impl Cache {
    /// The cache in `dir` (made when first written).
    pub fn open(dir: &Path) -> Cache {
        let memo = std::fs::read(dir.join(MEMO)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Cache { dir: dir.to_path_buf(), memo: Mutex::new(memo) }
    }

    /// The cache's folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// A file's contents' hash: from the memo when its size and time are those it had, else read.
    pub fn hash_file(&self, path: &Path) -> Result<String> {
        let (size, mtime) = stat(path)?;
        let name = path.to_string_lossy().into_owned();
        if let Some(seen) = self.memo.lock().unwrap_or_else(|p| p.into_inner()).files.get(&name)
            && seen.size == size
            && seen.mtime == mtime
        {
            return Ok(seen.sha1.clone());
        }
        let sha1 = sha1_file(path)?;
        // (taken again after the read: a file changed while it was read gets a new time, and is read again next time)
        let (size2, mtime2) = stat(path)?;
        if (size2, mtime2) == (size, mtime) {
            let seen = Seen { size, mtime, sha1: sha1.clone() };
            self.memo.lock().unwrap_or_else(|p| p.into_inner()).files.insert(name, seen);
        }
        Ok(sha1)
    }

    /// A file put in `key` by its contents, named `name` (not its path: a romfs elsewhere keeps the keys).
    pub fn key_file(&self, key: &mut Key, name: &str, path: &Path) -> Result<()> {
        key.text(name, &self.hash_file(path)?);
        Ok(())
    }

    /// A romfs file put in `key` by its fingerprint, named by its path under the romfs.
    pub fn key_romfs(&self, key: &mut Key, romfs: &dyn crate::romfs::Romfs, rel: &str) -> Result<()> {
        key.text(rel, &romfs.fingerprint(rel)?);
        Ok(())
    }

    /// A file that may be missing put in `key`: its contents, or that it is missing.
    pub fn key_maybe_file(&self, key: &mut Key, name: &str, path: &Path) -> Result<()> {
        if path.is_file() {
            return self.key_file(key, name, path);
        }
        key.text(name, "(none)");
        Ok(())
    }

    /// Every file under `dir` whose name ends in `suffix` put in `key`, by its path under `dir` and its contents
    /// (nothing when `dir` is missing).
    pub fn key_tree(&self, key: &mut Key, name: &str, dir: &Path, suffix: &str) -> Result<()> {
        key.text(name, "tree");
        if !dir.is_dir() {
            return Ok(());
        }
        for rel in fsx::files_under(dir)? {
            let shown = rel.to_string_lossy().replace('\\', "/");
            if shown.ends_with(suffix) {
                self.key_file(key, &shown, &dir.join(&rel))?;
            }
        }
        Ok(())
    }

    fn stamp_path(&self, name: &str) -> PathBuf {
        self.dir.join("stamps").join(format!("{name}.json"))
    }

    /// Whether the outputs stamped `name` were made from `key` and are still as they were made.
    pub fn fresh(&self, name: &str, key: &Key) -> bool {
        let Some(stamp) =
            std::fs::read(self.stamp_path(name)).ok().and_then(|b| serde_json::from_slice::<Stamp>(&b).ok())
        else {
            return false;
        };
        stamp.key == key.hex()
            && stamp.outputs.iter().all(|(path, sha1)| self.hash_file(Path::new(path)).is_ok_and(|h| &h == sha1))
    }

    /// The outputs made from `key`, stamped `name`.
    pub fn record(&self, name: &str, key: &Key, outputs: &[PathBuf]) -> Result<()> {
        let mut out = Vec::with_capacity(outputs.len());
        for path in outputs {
            out.push((path.to_string_lossy().into_owned(), self.hash_file(path)?));
        }
        let stamp = Stamp { key: key.hex(), outputs: out };
        let bytes = serde_json::to_vec_pretty(&stamp).map_err(|e| build_core::Error::format(name, e.to_string()))?;
        fsx::write_atomic(&self.stamp_path(name), &bytes)?;
        self.save()
    }

    /// The stamp `name` removed: its outputs are made again next time (before they are changed or removed).
    pub fn forget(&self, name: &str) -> Result<()> {
        fsx::remove_file(&self.stamp_path(name))
    }

    /// The memo of hashes written, for the next build.
    pub fn save(&self) -> Result<()> {
        let bytes = {
            let memo = self.memo.lock().unwrap_or_else(|p| p.into_inner());
            serde_json::to_vec(&*memo).map_err(|e| build_core::Error::format(MEMO, e.to_string()))?
        };
        fsx::write_if_changed(&self.dir.join(MEMO), &bytes).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("build-assets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn an_output_stays_fresh_until_its_inputs_or_itself_change() {
        let d = scratch("cache");
        let input = d.join("in.txt");
        let output = d.join("out.txt");
        std::fs::write(&input, b"one").unwrap();
        std::fs::write(&output, b"made").unwrap();
        let cache = Cache::open(&d.join("cache"));
        let key = |cache: &Cache| {
            let mut k = Key::new();
            cache.key_file(&mut k, "in", &input).unwrap();
            k
        };
        assert!(!cache.fresh("x", &key(&cache)));
        cache.record("x", &key(&cache), std::slice::from_ref(&output)).unwrap();
        assert!(cache.fresh("x", &key(&cache)));
        // (a second build in the same folder: the memo read back)
        let again = Cache::open(&d.join("cache"));
        assert!(again.fresh("x", &key(&again)));
        std::fs::write(&input, b"three").unwrap();
        assert!(!again.fresh("x", &key(&again)), "an input changed");
        again.record("x", &key(&again), std::slice::from_ref(&output)).unwrap();
        std::fs::write(&output, b"edited").unwrap();
        assert!(!again.fresh("x", &key(&again)), "the output changed");
        again.record("x", &key(&again), std::slice::from_ref(&output)).unwrap();
        std::fs::remove_file(&output).unwrap();
        assert!(!again.fresh("x", &key(&again)), "the output is gone");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
