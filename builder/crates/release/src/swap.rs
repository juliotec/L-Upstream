//! A program replaced by a newer one while it may be running (`longshot-builder self-update`; the Longshot Launcher's
//! Core/SelfUpdate.cs Swap, Recover and WhileHeld, ported), and a Windows program made a windowed one.
//!
//! Linux lets a running program's file be replaced: the new one is renamed over it, one rename, so there is never a
//! moment without it (the running process keeps its own, unlinked file); the one before is copied aside as `.old`
//! first. Windows lets a running program be renamed but not overwritten: it goes aside as `<name>.old`, then the new
//! one takes its name, and the old one comes back if the new one can't. The `.old` is deleted at the next start
//! ([`tidy`]), when nothing runs from it any more (a delete that fails, an old copy still running, is tried again at
//! the start after); a start that finds no program but its `.old` (power lost between the two renames) puts it back.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// The program kept aside by a swap.
#[must_use]
pub fn old(current: &Path) -> PathBuf {
    with_suffix(current, ".old")
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// The waits between tries while Windows holds a file just written (an antivirus scanning it): about 10 s in all.
pub const HELD_WAITS_MS: [u64; 8] = [100, 250, 500, 1000, 2000, 2000, 2000, 2000];

/// A Windows failure of a file another process has open: a sharing or lock violation, or the access denied a rename
/// answers for a file an antivirus holds.
#[must_use]
pub fn is_held(e: &std::io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

/// `action` tried again while the file is held ([`is_held`]), waiting a little longer each time; any other failure,
/// or one held longer, returned.
pub fn while_held(mut action: impl FnMut() -> std::io::Result<()>, wait: &dyn Fn(Duration)) -> std::io::Result<()> {
    for ms in HELD_WAITS_MS {
        match action() {
            Err(e) if is_held(&e) => wait(Duration::from_millis(ms)),
            r => return r,
        }
    }
    action()
}

/// `staged` put in place of `current`, the one before kept as `current.old`. `windows`: Windows' way (rename aside
/// first), else Linux's (one rename over it). On Windows, the one before comes back when the new one can't take its
/// place.
pub fn swap(current: &Path, staged: &Path, windows: bool, wait: &dyn Fn(Duration)) -> std::io::Result<()> {
    let backup = old(current);
    if !windows {
        let part = with_suffix(&backup, ".part");
        std::fs::copy(current, &part)?;
        std::fs::rename(&part, &backup)?;
        return std::fs::rename(staged, current);
    }
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(current, &backup)?;
    match while_held(|| std::fs::rename(staged, current), wait) {
        Ok(()) => Ok(()),
        Err(e) => {
            if !current.exists() {
                std::fs::rename(&backup, current)?;
            }
            Err(e)
        }
    }
}

/// What a start finds left of an update, put right: no program but its `.old` (an update cut between its renames):
/// the `.old` back; else the `.old` of the last update deleted, when nothing holds it. What was done, for the log.
pub fn tidy(current: &Path) -> Option<String> {
    let backup = old(current);
    if !current.exists() && backup.is_file() {
        return std::fs::rename(&backup, current)
            .ok()
            .map(|()| format!("an update was cut short: {} put back", current.display()));
    }
    let _ = std::fs::remove_file(with_suffix(&backup, ".part"));
    if backup.is_file() && std::fs::remove_file(&backup).is_ok() {
        return Some(format!("the program before the last update ({}) deleted", backup.display()));
    }
    None
}

/// A Windows program's PE subsystem set to `to` (2: windowed, 3: console): the one before. That one field changes:
/// the image carries no signature, and Windows checks no checksum of a program.
pub fn set_subsystem(exe: &mut [u8], to: u16) -> Result<u16, String> {
    let u16_at = |b: &[u8], i: usize| b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
    let pe = exe.get(0x3c..0x40).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize);
    let pe = pe.filter(|&pe| exe.starts_with(b"MZ") && exe.get(pe..pe + 4) == Some(b"PE\0\0".as_slice()));
    let Some(pe) = pe else { return Err("the Windows program is not a PE image".into()) };
    // (the optional header after the 24 bytes of signature and file header; its subsystem at 68, PE32 or PE32+)
    let (optional, subsystem) = (pe + 24, pe + 24 + 68);
    if !matches!(u16_at(exe, optional), Some(0x10b | 0x20b)) {
        return Err("the Windows program has no optional header".into());
    }
    match u16_at(exe, subsystem) {
        Some(was @ (2 | 3)) => {
            exe[subsystem..subsystem + 2].copy_from_slice(&to.to_le_bytes());
            Ok(was)
        }
        other => Err(format!("the Windows program's subsystem is {other:?}, not a console's or a window's")),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn fake_pe(subsystem: u16) -> Vec<u8> {
        let mut v = vec![0u8; 0x200];
        v[..2].copy_from_slice(b"MZ");
        v[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        v[0x80..0x84].copy_from_slice(b"PE\0\0");
        v[0x80 + 24..0x80 + 26].copy_from_slice(&0x20bu16.to_le_bytes());
        v[0x80 + 24 + 68..0x80 + 24 + 70].copy_from_slice(&subsystem.to_le_bytes());
        v
    }

    #[test]
    fn the_subsystem_is_flipped_and_nothing_else() {
        let mut pe = fake_pe(3);
        let before = pe.clone();
        assert_eq!(set_subsystem(&mut pe, 2), Ok(3));
        assert_eq!(pe.iter().zip(&before).filter(|(a, b)| a != b).count(), 1);
        assert_eq!(set_subsystem(&mut pe, 3), Ok(2));
        assert_eq!(pe, before);
        assert!(set_subsystem(&mut b"ELF".to_vec(), 2).is_err());
    }

    fn run(windows: bool) {
        let d = crate::tests::scratch(&format!("swap-{windows}"));
        let (cur, new) = (d.join("Longshot.x86_64"), d.join("staged"));
        std::fs::write(&cur, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        swap(&cur, &new, windows, &|_| {}).unwrap();
        assert_eq!(std::fs::read(&cur).unwrap(), b"new");
        assert_eq!(std::fs::read(old(&cur)).unwrap(), b"old", "the one before kept aside");
        assert!(!new.exists());
        // (the next start deletes the .old)
        assert!(tidy(&cur).unwrap().contains("deleted"));
        assert!(!old(&cur).exists() && cur.exists());
        assert_eq!(tidy(&cur), None);
        // (a new one that can't take the place: the one before is back, Windows' way)
        if windows {
            let e = swap(&cur, &d.join("missing"), true, &|_| {});
            assert!(e.is_err());
            assert_eq!(std::fs::read(&cur).unwrap(), b"new", "the program is back");
        }
        // (cut between the renames: only the .old left; the start puts it back)
        std::fs::rename(&cur, old(&cur)).unwrap();
        assert!(tidy(&cur).unwrap().contains("put back"));
        assert_eq!(std::fs::read(&cur).unwrap(), b"new");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn linux_s_swap_is_one_rename_over_it() {
        run(false);
    }

    #[test]
    fn windows_swap_renames_aside_first() {
        run(true);
    }

    #[test]
    fn a_held_file_is_tried_again_then_given_up() {
        let tries = std::cell::Cell::new(0);
        let r = while_held(
            || {
                tries.set(tries.get() + 1);
                Err(std::io::Error::other("not held"))
            },
            &|_| {},
        );
        assert!(r.is_err());
        assert_eq!(tries.get(), 1, "a failure that is not a held file is not tried again");
    }
}
