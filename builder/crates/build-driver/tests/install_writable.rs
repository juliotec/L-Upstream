//! The builder's files installed from a read-only source (a Nix store, a packaged install, a read-only tools folder)
//! come out writable by the player, and an executable stays executable. A copy that took the read-only mode along left
//! the game folder's files read-only: the player could not edit them, and on Windows the next build could not replace
//! them (ported from tests/test_copy_writable.py, which checked make_game's copies).

use build_driver::folder::install;
use std::path::{Path, PathBuf};

fn dir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join("install_writable").join(name);
    if d.exists() {
        // (a read-only file from an earlier run: writable again before it is removed)
        for f in std::fs::read_dir(&d).unwrap().flatten() {
            let mut p = f.metadata().unwrap().permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            p.set_readonly(false);
            let _ = std::fs::set_permissions(f.path(), p);
        }
        std::fs::remove_dir_all(&d).unwrap();
    }
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn read_only(path: &Path, text: &str, exec: bool) {
    if let Ok(m) = std::fs::metadata(path) {
        let mut p = m.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(false);
        std::fs::set_permissions(path, p).unwrap();
    }
    std::fs::write(path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(if exec { 0o555 } else { 0o444 })).unwrap();
    }
    #[cfg(not(unix))]
    {
        let _ = exec;
        let mut p = std::fs::metadata(path).unwrap().permissions();
        p.set_readonly(true);
        std::fs::set_permissions(path, p).unwrap();
    }
}

#[test]
fn a_read_only_source_is_installed_writable() {
    let src = dir("src");
    let out = dir("out");
    read_only(&src.join("icon.png"), "icon", false);
    read_only(&src.join("Longshot"), "#!/bin/sh\n", true);
    let files = vec![(src.join("icon.png"), out.join("icon.png")), (src.join("Longshot"), out.join("Longshot"))];
    assert_eq!(install(&out, &files, &[], &[]).unwrap().copied, 2);
    for (_, dst) in &files {
        let p = std::fs::metadata(dst).unwrap().permissions();
        assert!(!p.readonly(), "{} is read-only", dst.display());
        std::fs::OpenOptions::new().append(true).open(dst).expect("the player can write it");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |f: &str| std::fs::metadata(out.join(f)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode("Longshot"), 0o755, "executable still, writable by its owner alone");
        assert_eq!(mode("icon.png"), 0o644);
    }

    // (a new build from the same read-only source replaces them, which a read-only file stopped on Windows)
    read_only(&src.join("icon.png"), "icon 2", false);
    let again = install(&out, &files, &[], &[]).unwrap();
    assert_eq!((again.copied, again.kept), (1, 1));
    assert_eq!(std::fs::read_to_string(out.join("icon.png")).unwrap(), "icon 2");
}
