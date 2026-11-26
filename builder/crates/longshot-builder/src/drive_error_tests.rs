//! A drive that cannot read its own files is said as the drive's fault, with its repair, not as a builder error to
//! report (issue 2063: listing an unpacked romfs folder raised "the file or directory is corrupted and unreadable").
//! A full drive is `disk_full`; any other error stays as it is. (The mapping as it is now: a change to which codes it
//! takes updates this file.)

use super::drive_error;
use build_core::Error;
use std::path::PathBuf;

fn io(path: &str, code: i32) -> Error {
    Error::Io { path: PathBuf::from(path), source: std::io::Error::from_raw_os_error(code) }
}

/// The codes this OS's drive faults are, with the reason each is said with.
fn faults() -> Vec<(i32, &'static str)> {
    if cfg!(windows) {
        vec![
            (23, "a read failed its data check"),
            (1117, "the drive reported an I/O error"),
            (1392, "the file or folder is corrupted and unreadable"),
            (1393, "the disk structure is corrupted and unreadable"),
        ]
    } else {
        vec![(5, "the drive reported an I/O error")]
    }
}

#[test]
fn a_drive_fault_is_the_drive_s_with_its_repair() {
    let path = if cfg!(windows) { r"D:\Games\Longshot\work\romfs\Tex" } else { "/games/Longshot/work/romfs/Tex" };
    let fix = if cfg!(windows) {
        "open a terminal as administrator and run \"chkdsk D: /f\""
    } else {
        "check the drive for errors"
    };
    for (code, reason) in faults() {
        let e = drive_error(io(path, code));
        assert_eq!(e.code(), "drive_error");
        assert_eq!(
            e.to_string(),
            format!(
                "Your drive could not read {path}: {reason}. This is a fault of the drive or its file system, not of \
                 Longshot or your game files. To repair it, {fix}, then build again. If it keeps happening the drive \
                 may be failing: build on another drive."
            )
        );
    }
}

#[test]
fn other_errors_stay_as_they_are() {
    // (not found, access denied: ENOENT, EACCES; a full drive, ENOSPC or ERROR_DISK_FULL, is `disk_full`: main.rs's
    // a_full_drive_is_disk_full_from_any_file_s_error)
    for code in [2, 13] {
        if faults().iter().any(|&(c, _)| c == code) {
            continue;
        }
        let e = drive_error(io("/x", code));
        assert_eq!(e.code(), "io", "{code}");
    }
    assert_eq!(drive_error(io("/x", if cfg!(windows) { 112 } else { 28 })).code(), "disk_full");
    assert_eq!(drive_error(Error::player("disk_full", "No room.")).to_string(), "No room.");
    assert_eq!(drive_error(Error::tool("clang", "exit 1")).code(), "tool_failed");
}
