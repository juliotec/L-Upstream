//! The profile's edit lock: held for the moment of a change to `profile.json` (the mods' list, a name, the copy), so the
//! builder serving a playing game's channel, the launcher and `mod` commands never write the file over each other.
//! Not the playing lock (`.longshot.lock`, held while the game runs): a profile that plays can be edited.
//!
//! The lock is a file made whole or not at all (`create_new`) holding its holder's process id and the time it was
//! taken. A lock whose holder is dead (Linux: no `/proc/<pid>`) or that has been held longer than [`STALE_AFTER`] (an
//! edit is a moment; on Windows, where the process is not looked up, this is the only rule) is stale: the next taker
//! takes it.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The lock's file in a profile folder.
pub const EDIT_LOCK: &str = ".longshot-edit.lock";

/// How long a lock may be held before it is taken as stale whoever holds it.
pub const STALE_AFTER: Duration = Duration::from_secs(30);

/// Why the lock was not taken.
#[derive(Debug)]
pub enum LockError {
    /// Another edit held it for the whole wait.
    Busy,
    /// The folder or the lock's file could not be used.
    Io(std::io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Busy => f.write_str("another change to the profile is going on"),
            LockError::Io(e) => write!(f, "the profile's edit lock could not be made: {e}"),
        }
    }
}

impl std::error::Error for LockError {}

/// The profile's edit lock, held until this is dropped.
#[derive(Debug)]
pub struct EditGuard {
    path: PathBuf,
    text: String,
}

/// A lock's text: `<pid> <seconds since 1970>`.
fn holder_text(pid: u32, at: SystemTime) -> String {
    format!("{pid} {}", at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()))
}

/// Whether the lock whose text is `text` is stale at `now`: unreadable, held too long, or its holder is gone.
fn stale(text: &str, now: SystemTime, alive: &dyn Fn(u32) -> bool) -> bool {
    let mut it = text.split_whitespace();
    let (Some(pid), Some(at)) =
        (it.next().and_then(|p| p.parse::<u32>().ok()), it.next().and_then(|t| t.parse::<u64>().ok()))
    else {
        return true;
    };
    let held = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()).saturating_sub(at);
    held > STALE_AFTER.as_secs() || !alive(pid)
}

/// Whether process `pid` runs (Linux: its `/proc` folder is there; elsewhere assumed so: only the time rule applies).
fn process_alive(pid: u32) -> bool {
    if cfg!(target_os = "linux") { Path::new(&format!("/proc/{pid}")).exists() } else { true }
}

/// How long [`edit_lock`] waits for another edit to finish.
pub const WAIT: Duration = Duration::from_secs(2);

/// The profile `profile`'s edit lock for the moment of a change to it, waiting up to [`WAIT`]: every edit of a
/// profile's `profile.json` or `trust.json` (the builder's, the launcher's, a mod tool's) takes it first, and lets it go
/// (drop) when done.
///
/// # Errors
/// [`LockError::Busy`] when another edit held it for the whole wait, [`LockError::Io`] when the file can't be made.
pub fn edit_lock(profile: &Path) -> Result<EditGuard, LockError> {
    EditGuard::take(profile, WAIT)
}

impl EditGuard {
    /// The profile `profile`'s edit lock, waiting up to `wait` for another edit to finish (the folder made when
    /// missing).
    ///
    /// # Errors
    /// [`LockError::Busy`] when it stayed held, [`LockError::Io`] when the file can't be made.
    pub fn take(profile: &Path, wait: Duration) -> Result<EditGuard, LockError> {
        EditGuard::take_with(profile, wait, &process_alive)
    }

    /// [`EditGuard::take`] with `alive` telling which processes run.
    fn take_with(profile: &Path, wait: Duration, alive: &dyn Fn(u32) -> bool) -> Result<EditGuard, LockError> {
        std::fs::create_dir_all(profile).map_err(LockError::Io)?;
        let path = profile.join(EDIT_LOCK);
        let deadline = std::time::Instant::now() + wait;
        loop {
            let text = holder_text(std::process::id(), SystemTime::now());
            let made = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).and_then(|mut f| {
                use std::io::Write;
                f.write_all(text.as_bytes())
            });
            match made {
                Ok(()) => return Ok(EditGuard { path, text }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(LockError::Io(e)),
            }
            // (held: a stale one is moved aside, one taker winning the move, and the loop makes it again)
            match std::fs::read_to_string(&path) {
                Ok(held) if stale(&held, SystemTime::now(), alive) => {
                    let aside = path.with_extension(format!("stale-{}", std::process::id()));
                    if std::fs::rename(&path, &aside).is_ok() {
                        let _ = std::fs::remove_file(&aside);
                    }
                    continue;
                }
                // (gone between: try again at once)
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                _ => {}
            }
            if std::time::Instant::now() >= deadline {
                return Err(LockError::Busy);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for EditGuard {
    fn drop(&mut self) {
        // (only our own: a lock taken as stale and made again by another is theirs)
        if std::fs::read_to_string(&self.path).is_ok_and(|t| t == self.text) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../target");
        let d = root.join("test-scratch").join(format!("edit-lock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn one_edit_at_a_time_and_the_lock_goes_when_dropped() {
        let d = scratch("one");
        let first = EditGuard::take(&d, Duration::ZERO).unwrap();
        assert!(matches!(EditGuard::take(&d, Duration::from_millis(30)), Err(LockError::Busy)));
        assert!(std::fs::read_to_string(d.join(EDIT_LOCK)).unwrap().starts_with(&std::process::id().to_string()));
        drop(first);
        assert!(!d.join(EDIT_LOCK).exists());
        drop(EditGuard::take(&d, Duration::ZERO).unwrap());
        // (a profile that is not there yet: made)
        drop(EditGuard::take(&d.join("new"), Duration::ZERO).unwrap());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_lock_whose_holder_is_dead_or_that_is_old_is_taken() {
        let d = scratch("stale");
        let now = SystemTime::now();
        // (a holder that is dead)
        std::fs::write(d.join(EDIT_LOCK), holder_text(4_000_000, now)).unwrap();
        assert!(EditGuard::take_with(&d, Duration::ZERO, &|_| false).is_ok(), "a dead holder's lock");
        // (a live holder, just now: held)
        std::fs::write(d.join(EDIT_LOCK), holder_text(4_000_000, now)).unwrap();
        assert!(matches!(EditGuard::take_with(&d, Duration::ZERO, &|_| true), Err(LockError::Busy)));
        // (a live holder for too long: stale)
        let old = now - STALE_AFTER - Duration::from_secs(5);
        std::fs::write(d.join(EDIT_LOCK), holder_text(4_000_000, old)).unwrap();
        assert!(EditGuard::take_with(&d, Duration::ZERO, &|_| true).is_ok(), "an old lock");
        // (a file that says nothing sensible: stale)
        std::fs::write(d.join(EDIT_LOCK), "garbage").unwrap();
        assert!(EditGuard::take_with(&d, Duration::ZERO, &|_| true).is_ok());
        // (the real check: this process lives, a made-up pid does not)
        assert!(process_alive(std::process::id()));
        assert!(!cfg!(target_os = "linux") || !process_alive(u32::MAX - 1));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_stale_lock_taken_by_another_is_not_removed_by_the_old_holder() {
        let d = scratch("theirs");
        let mine = EditGuard::take(&d, Duration::ZERO).unwrap();
        std::fs::write(d.join(EDIT_LOCK), holder_text(1, SystemTime::now())).unwrap();
        drop(mine);
        assert!(d.join(EDIT_LOCK).exists(), "the other's lock stays");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
