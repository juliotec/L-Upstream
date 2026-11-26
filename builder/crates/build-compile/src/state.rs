//! What the last build learned, kept for the next one (`<build>/compile_state.json`): each file's content hash by its
//! size, time and id, so an unchanged rebuild reads no source and no header again; each compiler's identity; each
//! unit's measured compile time and peak memory, which order the next compile and its countdown.
//!
//! A file whose size, time and id are the recorded ones has the recorded content, unless it was written less than
//! [`RACY_NS`] before the record was taken: a change within the clock's step keeps the time (git's "racily clean"
//! entries), so such a file is read again. The record is taken under the time its build started, before any file was
//! looked at. A record that cannot be read is no record: it costs a read of everything, never a wrong object.

use build_core::{IoContext, Result, fsx};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// The file's name in the build folder.
pub const FILE: &str = "compile_state.json";
/// The record's format; another is no record.
const SCHEME: u32 = 1;
/// How long before the record a file must have been written for its time to vouch for its content.
pub const RACY_NS: u128 = 2_000_000_000;

/// A file as the file system tells it: size, modification time (ns), and its id (inode; 0 where not told).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stat(pub u64, pub u128, pub u64);

impl Stat {
    /// `path`'s, or `None` when it is not there.
    pub fn of(path: &Path) -> Option<Stat> {
        let m = std::fs::metadata(path).ok()?;
        let mtime = m.modified().ok()?.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        Some(Stat(m.len(), mtime, inode(&m)))
    }
}

#[cfg(unix)]
fn inode(m: &std::fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::ino(m)
}

#[cfg(not(unix))]
fn inode(_: &std::fs::Metadata) -> u64 {
    0 // (std tells no file id on Windows: size and time alone, with the racy rule)
}

/// What is known of one file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileRecord {
    /// Its stat when it was hashed.
    pub stat: Stat,
    /// Its content's SHA-1.
    pub sha1: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    scheme: u32,
    written_ns: u128,
    #[serde(default)]
    files: HashMap<String, FileRecord>,
    #[serde(default)]
    compilers: HashMap<String, (Stat, String)>,
    #[serde(default)]
    seconds: HashMap<String, f64>,
    #[serde(default)]
    peak_mb: HashMap<String, u64>,
}

/// The state, shared by the compile's threads.
pub struct State {
    path: PathBuf,
    started_ns: u128,
    trusted_before: u128,
    inner: Mutex<Saved>,
}

fn now_ns() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos())
}

impl State {
    /// The state in `build`, as the last build left it (or empty).
    pub fn load(build: &Path) -> State {
        let started_ns = now_ns(); // (before any file is looked at: the time the next record is taken under)
        let path = build.join(FILE);
        let saved = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Saved>(&b).ok())
            .filter(|s| s.scheme == SCHEME)
            .unwrap_or_default();
        let trusted_before = saved.written_ns.saturating_sub(RACY_NS);
        State { path, started_ns, trusted_before, inner: Mutex::new(saved) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Saved> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The record of `path` while it still describes the file on disk, and the file's stat (`None`: not there).
    pub fn known(&self, path: &Path) -> (Option<FileRecord>, Option<Stat>) {
        let Some(stat) = Stat::of(path) else { return (None, None) };
        let key = path.to_string_lossy();
        let rec = self.lock().files.get(key.as_ref()).cloned();
        (rec.filter(|r| r.stat == stat && stat.1 < self.trusted_before), Some(stat))
    }

    /// `path`'s content hash, read and hashed only when the record does not vouch for it.
    pub fn sha1(&self, path: &Path) -> Result<String> {
        let before = match self.known(path) {
            (Some(rec), _) => return Ok(rec.sha1),
            (None, stat) => stat,
        };
        let bytes = std::fs::read(path).at(path)?;
        Ok(self.remember(path, &bytes, before))
    }

    /// `path`'s hash, `None` when it cannot be read (a header gone since: the key then differs).
    pub fn sha1_if_there(&self, path: &Path) -> Option<String> {
        self.sha1(path).ok()
    }

    /// The record of `path` for `bytes` just read from it (`before`: its stat before the read): its hash. Recorded only
    /// when the file's stat now is the one before: a file written while it was read would be recorded with its new
    /// stat and its old bytes' hash, and that record believed by every later build.
    pub fn remember(&self, path: &Path, bytes: &[u8], before: Option<Stat>) -> String {
        let sha1 = sha1_smol::Sha1::from(bytes).digest().to_string();
        if let Some(stat) = Stat::of(path).filter(|s| before.as_ref() == Some(s)) {
            let mut s = self.lock();
            let key = path.to_string_lossy().into_owned();
            s.files.insert(key, FileRecord { stat, sha1: sha1.clone() });
        }
        sha1
    }

    /// A compiler's identity, from the record while the compiler's file is as it was.
    pub fn compiler(&self, path: &Path, f: impl FnOnce() -> Result<String>) -> Result<String> {
        let stat = Stat::of(path);
        let key = path.to_string_lossy().into_owned();
        if let (Some(stat), Some((s, id))) = (stat, self.lock().compilers.get(&key))
            && *s == stat
            && stat.1 < self.trusted_before
        {
            return Ok(id.clone());
        }
        let id = f()?;
        if let Some(stat) = stat {
            self.lock().compilers.insert(key, (stat, id.clone()));
        }
        Ok(id)
    }

    /// The seconds a unit took when it was last compiled here.
    pub fn seconds(&self, unit: &str) -> Option<f64> {
        self.lock().seconds.get(unit).copied()
    }

    /// Notes the seconds a unit took.
    pub fn set_seconds(&self, unit: &str, seconds: f64) {
        self.lock().seconds.insert(unit.to_string(), seconds);
    }

    /// The most memory a unit's compiler was seen to hold when it was last compiled here, in MB.
    pub fn peak_mb(&self, unit: &str) -> Option<u64> {
        self.lock().peak_mb.get(unit).copied()
    }

    /// Notes the most memory a unit's compiler was seen to hold.
    pub fn set_peak_mb(&self, unit: &str, mb: u64) {
        self.lock().peak_mb.insert(unit.to_string(), mb);
    }

    /// Writes the state for the next build, keeping the records of files still there. A failure only costs the next
    /// build a read of everything.
    pub fn save(&self) -> Result<()> {
        let mut s = self.lock();
        s.scheme = SCHEME;
        s.written_ns = self.started_ns;
        s.files.retain(|p, _| Path::new(p).exists());
        let bytes = serde_json::to_vec(&*s).map_err(|e| build_core::Error::format("compile state", e.to_string()))?;
        fsx::write_atomic(&self.path, &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_written_while_it_was_read_is_not_recorded() {
        let d = crate::testdir("state-moved");
        let f = d.join("a.h");
        std::fs::write(&f, b"one").unwrap();
        let before = Stat::of(&f);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&f, b"two!").unwrap(); // (after the stat, as if during the read)
        let s = State::load(&d);
        s.remember(&f, b"one", before);
        assert!(s.lock().files.is_empty(), "the new stat with the old bytes' hash was recorded");
        s.remember(&f, b"two!", Stat::of(&f));
        assert_eq!(s.lock().files.len(), 1);
    }

    #[test]
    fn a_file_is_read_again_unless_its_record_is_older_than_the_racy_step() {
        let d = crate::testdir("state");
        let f = d.join("a.c");
        std::fs::write(&f, b"int a;").unwrap();
        let s = State::load(&d);
        let h = s.sha1(&f).unwrap();
        assert_eq!(h, sha1_smol::Sha1::from(b"int a;").digest().to_string());
        s.save().unwrap();
        // (the record was taken now: the file, written just before, is not vouched for yet)
        let again = State::load(&d);
        assert!(again.known(&f).0.is_none());
        assert_eq!(again.sha1(&f).unwrap(), h);
    }
}
