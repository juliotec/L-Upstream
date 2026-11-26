//! One build at a time in a work folder, and in a game folder.
//!
//! Two builds in one folder share its lift, its object cache and its lists: the second finds the lift done and
//! compiles again every object the first has not cached yet, and one can delete or cut short the other's files. So the
//! second stops at once, saying so. A lock, not a file that says so: it goes with the process that held it however that
//! ended, so a stopped build never keeps the next one out. Two builds into one game folder from two work folders
//! would swap their files in over each other's, and one's romfs index over the other's: the game folder has a lock of
//! its own ([`GameLock`]).

use crate::error::IoContext;
use crate::{Error, Result};
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

/// The lock's file in the work folder.
pub const FILE: &str = "longshot-builder.lock";

/// A work folder held by this build, until this is dropped (or the process ends).
#[derive(Debug)]
pub struct WorkLock {
    _file: File,
}

impl WorkLock {
    /// The work folder `work` held, or the player told another build is using it.
    pub fn take(work: &Path) -> Result<WorkLock> {
        Ok(WorkLock { _file: hold(work, FILE, "work_folder_busy", "work folder")? })
    }
}

/// The lock's file in a game folder.
pub const GAME_FILE: &str = ".longshot-builder.lock";

/// A game folder held by this build (its files swapped in, its romfs index written), until this is dropped.
#[derive(Debug)]
pub struct GameLock {
    _file: File,
}

impl GameLock {
    /// The game folder `out` held (made if missing), or the player told another build is writing it.
    pub fn take(out: &Path) -> Result<GameLock> {
        Ok(GameLock { _file: hold(out, GAME_FILE, "game_folder_busy", "game folder")? })
    }
}

/// The lock's file in a profile folder: one copy of the game per profile ([`ProfileLock`]).
pub const PROFILE_FILE: &str = ".longshot.lock";
/// The file beside it that names the process holding it (its pid, as text): Windows keeps a locked file's bytes from
/// every other process, so the pid can't be in the lock's own file.
pub const PROFILE_PID_FILE: &str = ".longshot.pid";

/// A profile held by the builder that plays it (while it builds and while the game runs), until this is dropped or the
/// process ends however it ends: a second start for the same profile finds it held and starts no second game writing
/// the same saves. Other profiles are other locks: they play at once.
#[derive(Debug)]
pub struct ProfileLock {
    _file: File,
}

impl ProfileLock {
    /// The profile `profile` held (the folder made when missing), its holder's pid noted beside the lock; `None` when
    /// another process holds it.
    pub fn take(profile: &Path) -> Result<Option<ProfileLock>> {
        std::fs::create_dir_all(profile).at(profile)?;
        let path = profile.join(PROFILE_FILE);
        let file = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&path).at(&path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(e)) => return Err(Error::io(&path, e)),
        }
        // (best effort: without it a second start can't find the game's window to bring forward, and nothing else)
        let _ = crate::fsx::write_atomic(&profile.join(PROFILE_PID_FILE), std::process::id().to_string().as_bytes());
        Ok(Some(ProfileLock { _file: file }))
    }

    /// Whether another process holds profile `profile` now (its game running, or being built for it). Only reads: no
    /// folder or file is made, and a profile with no lock file is not held.
    pub fn held(profile: &Path) -> bool {
        let Ok(file) = OpenOptions::new().read(true).open(profile.join(PROFILE_FILE)) else { return false };
        matches!(file.try_lock_shared(), Err(TryLockError::WouldBlock))
    }

    /// The pid of the process that last held profile `profile` (the one holding it, when [`ProfileLock::held`]).
    pub fn holder(profile: &Path) -> Option<u32> {
        std::fs::read_to_string(profile.join(PROFILE_PID_FILE)).ok()?.trim().parse().ok()
    }
}

fn hold(dir: &Path, name: &str, code: &'static str, what: &str) -> Result<File> {
    std::fs::create_dir_all(dir).at(dir)?;
    let path = dir.join(name);
    let file = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&path).at(&path)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(Error::player(
            code,
            format!(
                "Another build is using the {what} {}. Wait for it to end (or stop it), then build again.",
                dir.display()
            ),
        )),
        Err(TryLockError::Error(e)) => Err(Error::io(&path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_build_is_refused_until_the_first_lets_go() {
        // (once let go, the lock is taken again; a test of this process that starts a program at that moment holds a
        // copy of the file between its fork and its exec, which closes it, so a few tries are allowed: that window, not
        // a lock kept, under a loaded machine)
        fn again<T>(take: impl Fn() -> Result<T>) -> T {
            for _ in 0..50 {
                if let Ok(lock) = take() {
                    return lock;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            take().unwrap()
        }
        let d = std::env::temp_dir().join(format!("build-core-lock-{}", std::process::id()));
        let first = WorkLock::take(&d).unwrap();
        assert!(matches!(WorkLock::take(&d), Err(Error::Player { code: "work_folder_busy", .. })));
        drop(first);
        drop(again(|| WorkLock::take(&d)));
        let game = again(|| GameLock::take(&d));
        assert!(matches!(GameLock::take(&d), Err(Error::Player { code: "game_folder_busy", .. })));
        drop(game);
        drop(again(|| GameLock::take(&d)));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A folder of the test's own, under the target folder.
    fn scratch(name: &str) -> std::path::PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/test-scratch")
            .join(format!("lock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn one_copy_per_profile_among_threads_and_other_profiles_at_once() {
        let d = scratch("profile-threads");
        let (ann, bo) = (d.join("ann"), d.join("bo"));
        assert!(!ProfileLock::held(&ann), "no lock file yet: not held, and nothing made");
        assert!(!ann.exists());
        let first = ProfileLock::take(&ann).unwrap().expect("free");
        assert_eq!(ProfileLock::holder(&ann), Some(std::process::id()));
        // (a second start, on another thread: refused while the first holds it; another profile plays at once)
        let (a, b) = (ann.clone(), bo.clone());
        let (second, other, seen) = std::thread::spawn(move || {
            (ProfileLock::take(&a).unwrap().is_none(), ProfileLock::take(&b).unwrap(), ProfileLock::held(&a))
        })
        .join()
        .unwrap();
        assert!(second && seen, "the same profile: held");
        assert!(other.is_some(), "another profile: its own lock");
        drop(first);
        assert!(!ProfileLock::held(&ann));
        assert!(ProfileLock::take(&ann).unwrap().is_some(), "let go: the next start takes it");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The variable that makes [`profile_lock_holder`] hold a profile's lock (its folder).
    const HOLD: &str = "BUILD_CORE_TEST_HOLD_PROFILE";

    /// Not a test alone: [`one_copy_per_profile_between_processes`] starts this test's program on it, with [`HOLD`],
    /// as a second process that holds the profile until it is killed.
    #[test]
    #[ignore = "a helper: one_copy_per_profile_between_processes runs it"]
    fn profile_lock_holder() {
        let Some(dir) = std::env::var_os(HOLD) else { return };
        let _held = ProfileLock::take(Path::new(&dir)).unwrap().expect("free");
        println!("held");
        std::thread::sleep(std::time::Duration::from_secs(120));
    }

    #[test]
    fn one_copy_per_profile_between_processes() {
        use std::io::BufRead;
        let d = scratch("profile-processes");
        let ann = d.join("ann");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["lock::tests::profile_lock_holder", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
            .env(HOLD, &ann)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut lines = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
        // (libtest says the test's name first, on the same line)
        assert!(lines.any(|l| l.is_ok_and(|l| l.trim_end().ends_with("held"))), "the other process holds it");
        assert!(ProfileLock::held(&ann));
        assert_eq!(ProfileLock::holder(&ann), Some(child.id()), "its pid noted beside the lock");
        assert!(ProfileLock::take(&ann).unwrap().is_none(), "a second start for the same profile");
        assert!(ProfileLock::take(&d.join("bo")).unwrap().is_some(), "another profile");
        // (the holder killed, as a crash ends it: the lock goes with it)
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!ProfileLock::held(&ann));
        assert!(ProfileLock::take(&ann).unwrap().is_some());
        let _ = std::fs::remove_dir_all(&d);
    }
}
