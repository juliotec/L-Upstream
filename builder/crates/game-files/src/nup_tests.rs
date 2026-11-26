//! A drive that fills during an extraction is said as a full drive, with the file, its size, the folder and its free
//! space, not as the bare I/O error; any other refused write stays the error it is.

use super::out_of_space;
use std::path::Path;

/// The code each OS's full drive is (ENOSPC; ERROR_DISK_FULL and ERROR_HANDLE_DISK_FULL).
const FULL: &[i32] = if cfg!(windows) { &[112, 39] } else { &[28] };

#[test]
fn a_full_drive_is_said_as_one() {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dst = folder.join("romfs.bin");
    for &code in FULL {
        let e = out_of_space(&std::io::Error::from_raw_os_error(code), &dst, "romfs/Tex/a.zs", 1234)
            .expect("a full drive is said");
        assert_eq!(e.code(), "disk_full");
        let text = e.to_string();
        let start =
            format!("The drive is full: no room to write romfs/Tex/a.zs (1234 bytes) into {} (", folder.display());
        assert!(text.starts_with(&start), "{text}");
        assert!(text.contains(" GB free there") || text.contains("free space unknown"), "{text}");
        assert!(
            text.ends_with("). Free some space, or choose a game folder on another drive, and build again."),
            "{text}"
        );
    }
}

#[test]
fn another_refused_write_is_not_a_full_drive() {
    let dst = Path::new(env!("CARGO_MANIFEST_DIR")).join("x");
    for e in [
        std::io::Error::from_raw_os_error(if cfg!(windows) { 5 } else { 13 }),
        std::io::Error::other("broken pipe"),
        std::io::Error::from_raw_os_error(if cfg!(windows) { 28 } else { 112 }),
    ] {
        assert!(out_of_space(&e, &dst, "a", 1).is_none(), "{e}");
    }
}
